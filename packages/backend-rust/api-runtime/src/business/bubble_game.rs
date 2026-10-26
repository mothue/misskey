/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */
//! Complete Bubble Game ranking over the source's two statement snapshots.
pub use super::entity_layout::LayoutFailure as BubbleRankingLayoutFailure;
use super::{
    Error,
    entity_layout::{column_name, compile_query, validated_layout},
    notes,
};
use crate::Runtime;
use misskey_api_validation::{ParseError, Value};
use misskey_backend_api_auth::{ColumnKind, Context, EntityColumn, postgres::quote_identifier};
use misskey_backend_postgres::QueryError;
use std::{
    collections::HashMap,
    fmt,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio_postgres::types::ToSql;

const RANKING_WINDOW_MS: i64 = 604_800_000;
const SELECT_IDS_SQL: &str = "SELECT DISTINCT d.record_id,d.record_score FROM (SELECT r.{id} AS record_id,r.{score} AS record_score FROM {table} r LEFT JOIN {user_table} u ON u.{user_id}=r.{author_id} WHERE r.{mode}=$1 AND r.{seeded_at}>$2) d ORDER BY d.record_score DESC,d.record_id ASC LIMIT 10";
const LOAD_SQL: &str = "SELECT r.{id},r.{score},u.{user_id},CASE WHEN u.{user_id} IS NULL THEN NULL ELSE {user_projection} END::text FROM {table} r LEFT JOIN {user_table} u ON u.{user_id}=r.{author_id} WHERE r.{mode}=$1 AND r.{seeded_at}>$2 AND r.{id} IN (";
const LOAD_ORDER_SQL: &str = ") ORDER BY r.{score} DESC";
// Actual apiEntityLayout export includes all selected properties; unused values
// need metadata validation, not hydration or new filters.
const RECORD_PROPERTIES: &[(&str, ColumnKind)] = &[
    ("id", ColumnKind::Json),
    ("userId", ColumnKind::Json),
    ("seededAt", ColumnKind::Date),
    ("seed", ColumnKind::Json),
    ("gameVersion", ColumnKind::Json),
    ("gameMode", ColumnKind::Json),
    ("score", ColumnKind::Json),
    ("logs", ColumnKind::Json),
    ("isVerified", ColumnKind::Json),
];

#[derive(Clone, Copy, Debug)]
pub(crate) enum Stage {
    Acquire,
    SelectIds,
    LoadJoined,
}
impl Stage {
    fn name(self) -> &'static str {
        match self {
            Self::Acquire => "acquire",
            Self::SelectIds => "select_ids",
            Self::LoadJoined => "load_joined",
        }
    }
}

#[derive(Debug)]
pub(crate) enum BubbleRankingFailure {
    InvalidParameter,
    ClockRange,
    Query {
        stage: Stage,
        source: QueryError,
    },
    Row {
        stage: Stage,
        source: tokio_postgres::Error,
    },
    Json {
        source: ParseError,
    },
    Pack {
        source: Box<Error>,
    },
}
impl BubbleRankingFailure {
    pub(super) fn report(&self) {
        let (stage, kind) = match self {
            Self::InvalidParameter => ("parameter", "invalid_parameter"),
            Self::ClockRange => ("cutoff", "clock_range"),
            Self::Query { stage, source } => {
                let kind = match source {
                    QueryError::Server { .. } => "query_server",
                    QueryError::QueryTimeout => "query_timeout",
                    QueryError::CheckoutTimeout => "checkout_timeout",
                    QueryError::Fault(_) => "query_fault",
                    QueryError::Unavailable => "unavailable",
                };
                (stage.name(), kind)
            }
            Self::Row { stage, .. } => (stage.name(), "row_decode"),
            Self::Json { .. } => ("load_joined", "row_json"),
            Self::Pack { .. } => ("pack_loaded", "user_lite"),
        };
        tracing::error!(
            operation = "bubble-game.ranking",
            stage,
            error.kind = kind,
            "endpoint failed"
        );
    }
}
impl fmt::Display for BubbleRankingFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("native Bubble Game ranking failed")
    }
}
impl std::error::Error for BubbleRankingFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Query { source, .. } => Some(source),
            Self::Row { source, .. } => Some(source),
            Self::Json { source } => Some(source),
            // The existing business Error is retained verbatim and projected
            // by its own response boundary; it has no std::error::Error impl.
            Self::Pack { .. } | Self::InvalidParameter | Self::ClockRange => None,
        }
    }
}

pub(crate) struct BubbleRankingQueries {
    select_ids: String,
    load_prefix: String,
    load_order: String,
}
impl BubbleRankingQueries {
    pub(crate) fn new(
        layouts: &HashMap<String, Vec<EntityColumn>>,
    ) -> Result<Self, BubbleRankingLayoutFailure> {
        let (table, columns) = validated_layout(layouts, "bubble_game_record", RECORD_PROPERTIES)?;
        let (user_table, user_columns) = layouts
            .get_key_value("user")
            .ok_or(BubbleRankingLayoutFailure::MissingLayout)?;
        let replacements = [
            ("table", quote_identifier(table)),
            ("id", column_name(columns, "id")?),
            ("score", column_name(columns, "score")?),
            ("mode", column_name(columns, "gameMode")?),
            ("seeded_at", column_name(columns, "seededAt")?),
            ("author_id", column_name(columns, "userId")?),
            ("user_table", quote_identifier(user_table)),
            ("user_id", column_name(user_columns, "id")?),
            (
                "user_projection",
                notes::user_lite_projection(user_columns, "u")?,
            ),
        ];
        Ok(Self {
            select_ids: compile_query(SELECT_IDS_SQL, &replacements)?,
            load_prefix: compile_query(LOAD_SQL, &replacements)?,
            load_order: compile_query(LOAD_ORDER_SQL, &replacements)?,
        })
    }

    fn load_sql(&self, count: usize) -> String {
        // Installed TypeORM uses scalar binds for the selected IDs. Keep the
        // original score-only response ordering, rather than membership order.
        let slots = (3..count + 3)
            .map(|slot| format!("${slot}"))
            .collect::<Vec<_>>()
            .join(",");
        format!("{}{slots}{}", self.load_prefix, self.load_order)
    }
}

fn game_mode(params: &Value) -> Result<String, Error> {
    let units = params
        .get("gameMode")
        .and_then(Value::as_utf16)
        .ok_or(Error::BubbleRanking(BubbleRankingFailure::InvalidParameter))?;
    // pg-protocol Bind uses Buffer UTF-8 conversion: isolated UTF-16 surrogates
    // become U+FFFD only at this DB boundary. Common validation is unchanged.
    Ok(String::from_utf16_lossy(units))
}

fn cutoff(now: i64) -> Result<SystemTime, Error> {
    let millis = now
        .checked_sub(RANKING_WINDOW_MS)
        .ok_or(Error::BubbleRanking(BubbleRankingFailure::ClockRange))?;
    let delta = Duration::from_millis(millis.unsigned_abs());
    let timestamp = if millis >= 0 {
        UNIX_EPOCH.checked_add(delta)
    } else {
        UNIX_EPOCH.checked_sub(delta)
    };
    timestamp.ok_or(Error::BubbleRanking(BubbleRankingFailure::ClockRange))
}

struct Record {
    id: String,
    score: i32,
    user_id: Option<String>,
}
struct LoadedRanking {
    records: Vec<Record>,
    users: Vec<Value>,
}
fn row_error(stage: Stage, source: tokio_postgres::Error) -> Error {
    Error::BubbleRanking(BubbleRankingFailure::Row { stage, source })
}
fn decode_rows(rows: &[tokio_postgres::Row]) -> Result<LoadedRanking, Error> {
    let mut records = Vec::with_capacity(rows.len());
    let mut users = Vec::with_capacity(rows.len());
    for row in rows {
        let stage = Stage::LoadJoined;
        let record = Record {
            id: row.try_get(0).map_err(|source| row_error(stage, source))?,
            score: row.try_get(1).map_err(|source| row_error(stage, source))?,
            user_id: row.try_get(2).map_err(|source| row_error(stage, source))?,
        };
        let text: String = row.try_get(3).map_err(|source| row_error(stage, source))?;
        let user = Value::parse_json(&text)
            .map_err(|source| Error::BubbleRanking(BubbleRankingFailure::Json { source }))?;
        records.push(record);
        users.push(user);
    }
    Ok(LoadedRanking { records, users })
}

async fn select_records(
    runtime: &Runtime,
    mode: &str,
    timestamp: SystemTime,
) -> Result<LoadedRanking, Error> {
    let queries = &runtime.bubble_ranking_queries;
    let mut read = runtime.database.read_lease().await.map_err(|source| {
        Error::BubbleRanking(BubbleRankingFailure::Query {
            stage: Stage::Acquire,
            source,
        })
    })?;
    let selected = read
        .query_typed(&queries.select_ids, &[&mode, &timestamp])
        .await
        .map_err(|source| {
            Error::BubbleRanking(BubbleRankingFailure::Query {
                stage: Stage::SelectIds,
                source,
            })
        })?;
    let ids = selected
        .iter()
        .map(|row| {
            row.try_get::<_, String>(0)
                .map_err(|source| row_error(Stage::SelectIds, source))
        })
        .collect::<Result<Vec<_>, _>>()?;
    if ids.is_empty() {
        return Ok(LoadedRanking {
            records: Vec::new(),
            users: Vec::new(),
        });
    }
    let mut params: Vec<&(dyn ToSql + Sync)> = vec![&mode, &timestamp];
    params.extend(ids.iter().map(|id| id as &(dyn ToSql + Sync)));
    let rows = read
        .query_typed(&queries.load_sql(ids.len()), &params)
        .await
        .map_err(|source| {
            Error::BubbleRanking(BubbleRankingFailure::Query {
                stage: Stage::LoadJoined,
                source,
            })
        })?;
    // TypeORM releases the ranking QueryRunner before calling packMany. UserLite
    // helpers may themselves read, so never hold this source lease while packing.
    drop(read);
    decode_rows(&rows)
}

fn assemble(records: Vec<Record>, users: &[Value]) -> Value {
    Value::Array(
        records
            .into_iter()
            .map(|record| {
                let user = record.user_id.as_deref().and_then(|id| {
                    let expected = Value::string(id);
                    users.iter().find(|user| user.get("id") == Some(&expected))
                });
                // ranking.ts uses users.find(...), including first-match duplicates and
                // undefined omission. Do not manufacture a null user or sort tied rows.
                Value::object([
                    ("id", Value::string(&record.id)),
                    ("score", Value::Number(f64::from(record.score))),
                    ("user", user.cloned().unwrap_or(Value::Undefined)),
                ])
            })
            .collect(),
    )
}

pub(super) async fn ranking(runtime: &Runtime, params: &Value) -> Result<Value, Error> {
    // The callback reads Date.now exactly once for its seven-day predicate;
    // online-status clocks remain owned by the shared UserLite formatter.
    let timestamp = cutoff(crate::now_ms())?;
    let loaded = select_records(runtime, &game_mode(params)?, timestamp).await?;
    let anonymous = Context {
        user: None,
        token: None,
        access_token_reduced: false,
    };
    let users = notes::pack_loaded_user_lites(runtime, &loaded.users, &anonymous)
        .await
        .map_err(|source| {
            Error::BubbleRanking(BubbleRankingFailure::Pack {
                source: Box::new(source),
            })
        })?;
    Ok(assemble(loaded.records, &users))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source() -> Value {
        // The actual source fixture contains lone surrogates. Its JS strings
        // must survive parsing before any DB-boundary replacement is tested.
        Value::parse_json(include_str!(
            "notes/fixtures/bubble-ranking-source-oracle.json"
        ))
        .unwrap()
    }
    fn array(value: &Value) -> &[Value] {
        let Value::Array(values) = value else {
            panic!("source array")
        };
        values
    }

    #[test]
    fn source_cutoff_is_one_strict_seven_day_predicate() {
        let oracle = source();
        let case = &array(oracle.get("cases").unwrap())[0];
        let timestamp = cutoff(1_800_000_000_123).unwrap();
        assert_eq!(
            timestamp.duration_since(UNIX_EPOCH).unwrap().as_millis(),
            case.get("cutoff")
                .unwrap()
                .stringify()
                .unwrap()
                .parse::<u128>()
                .unwrap()
        );
        assert_eq!(case.get("clockReads").unwrap().as_number(), Some(1.));
        assert_eq!(
            case.get("repositoryPredicate").unwrap(),
            &Value::string("moreThan")
        );
    }

    #[test]
    fn database_mode_conversion_matches_installed_pg_without_changing_input() {
        let oracle = source();
        for case in array(oracle.get("utf16Transport").unwrap()) {
            let units: Vec<u16> =
                serde_json::from_str(&case.get("inputUtf16").unwrap().stringify().unwrap())
                    .unwrap();
            let params = Value::object([("gameMode", Value::String(units.clone()))]);
            assert_eq!(
                game_mode(&params).unwrap(),
                case.get("encodedString").unwrap().to_utf8().unwrap()
            );
            assert_eq!(
                params.get("gameMode").unwrap().as_utf16(),
                Some(units.as_slice())
            );
        }
    }

    #[test]
    fn assembly_matches_source_first_find_order_and_signed_scores() {
        let oracle = source();
        let expected = array(oracle.get("cases").unwrap())[1]
            .get("result")
            .unwrap();
        let rows = array(expected);
        let records = rows
            .iter()
            .map(|row| Record {
                id: row.get("id").unwrap().to_utf8().unwrap(),
                score: row
                    .get("score")
                    .unwrap()
                    .stringify()
                    .unwrap()
                    .parse()
                    .unwrap(),
                user_id: Some(
                    row.get("user")
                        .unwrap()
                        .get("id")
                        .unwrap()
                        .to_utf8()
                        .unwrap(),
                ),
            })
            .collect();
        let users = [
            rows[0].get("user").unwrap().clone(),
            rows[1].get("user").unwrap().clone(),
            Value::object([
                ("id", Value::string("duplicate")),
                ("oracleMarker", Value::string("third")),
            ]),
        ];
        assert_eq!(&assemble(records, &users), expected);
    }

    #[test]
    fn assembly_unmatched_sentinel_omits_user_as_source_javascript_does() {
        // This artificial pack marker proves assembly only, not a legal stored
        // row, actual UserLite output or missing-user public error behavior.
        let oracle = source();
        let expected = array(oracle.get("cases").unwrap())[2]
            .get("result")
            .unwrap();
        let rows = vec![Record {
            id: "record-z".to_owned(),
            score: i32::MIN,
            user_id: Some("duplicate".to_owned()),
        }];
        let assembled = assemble(
            rows,
            &[Value::object([("id", Value::string("different-id"))])],
        );
        assert_eq!(
            assembled.stringify().unwrap(),
            expected.stringify().unwrap()
        );
    }
}
