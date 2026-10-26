/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */
//! Authenticated state with typed rows and the source's three independent reads.
pub use super::super::entity_layout::LayoutFailure as StateLayoutFailure;
use super::super::entity_layout::{column_name, compile_query, projected_layout};
use super::{Error, Value};
use crate::Runtime;
use misskey_backend_api_auth::{Context, EntityColumn, postgres::quote_identifier};
use misskey_backend_postgres::QueryError;
use std::{collections::HashMap, fmt, future::Future};

const NOTE_SQL: &str = "SELECT n.{id},n.{thread_id} FROM {table} n WHERE n.{id}=$1 LIMIT 1";
const COUNT_SQL: &str =
    "SELECT COUNT(*) <> 0 FROM {table} s WHERE s.{user_id}=$1 AND s.{target_id}=$2";

#[derive(Clone, Copy, Debug)]
pub(crate) enum Operation {
    Lookup,
    Favorite,
    Thread,
}
impl Operation {
    fn name(self) -> &'static str {
        match self {
            Self::Lookup => "notes.state.lookup",
            Self::Favorite => "notes.state.favorite",
            Self::Thread => "notes.state.thread",
        }
    }
}
#[derive(Clone, Copy, Debug)]
pub(crate) enum ShapeKind {
    InvalidParameter,
    InvalidPrincipal,
    QueryShape,
}
#[derive(Debug)]
pub(crate) enum StateFailure {
    Query {
        operation: Operation,
        source: QueryError,
    },
    Row {
        operation: Operation,
        source: tokio_postgres::Error,
    },
    Shape {
        operation: Operation,
        kind: ShapeKind,
    },
}
impl StateFailure {
    pub(crate) fn report(&self) {
        match self {
            Self::Query { operation, .. } => tracing::error!(
                operation = operation.name(),
                error.kind = "query",
                "endpoint failed"
            ),
            Self::Row { operation, .. } => tracing::error!(
                operation = operation.name(),
                error.kind = "row_decode",
                "endpoint failed"
            ),
            Self::Shape { operation, kind } => {
                tracing::error!(operation = operation.name(), error.kind = ?kind, "endpoint failed")
            }
        }
    }
}
impl fmt::Display for StateFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("native note state read failed")
    }
}
impl std::error::Error for StateFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Query { source, .. } => Some(source),
            Self::Row { source, .. } => Some(source),
            Self::Shape { .. } => None,
        }
    }
}
fn shape(operation: Operation, kind: ShapeKind) -> Error {
    Error::NoteState(StateFailure::Shape { operation, kind })
}

pub(crate) struct StateQueries {
    note: String,
    favorite: String,
    thread: String,
}
impl StateQueries {
    pub(crate) fn new(
        layouts: &HashMap<String, Vec<EntityColumn>>,
    ) -> Result<Self, StateLayoutFailure> {
        let (table, columns) = projected_layout(layouts, "note", &["id", "threadId"])?;
        let note = compile_query(
            NOTE_SQL,
            &[
                ("table", quote_identifier(table)),
                ("id", column_name(columns, "id")?),
                ("thread_id", column_name(columns, "threadId")?),
            ],
        )?;
        let favorite = count_query(layouts, "note_favorite", "noteId")?;
        let thread = count_query(layouts, "note_thread_muting", "threadId")?;
        Ok(Self {
            note,
            favorite,
            thread,
        })
    }
}
fn count_query(
    layouts: &HashMap<String, Vec<EntityColumn>>,
    table_name: &str,
    target: &str,
) -> Result<String, StateLayoutFailure> {
    let (table, columns) = projected_layout(layouts, table_name, &["userId", target])?;
    compile_query(
        COUNT_SQL,
        &[
            ("table", quote_identifier(table)),
            ("user_id", column_name(columns, "userId")?),
            ("target_id", column_name(columns, target)?),
        ],
    )
}

#[derive(Clone)]
struct StateNote {
    id: String,
    thread_id: Option<String>,
}
impl StateNote {
    fn thread_id(&self) -> &str {
        // note.threadId ?? note.id: an empty stored string is not null.
        self.thread_id.as_deref().unwrap_or(&self.id)
    }
}

fn missing_note(note_id: &str) -> Error {
    let Ok(encoded) = serde_json::to_string(note_id) else {
        return Error::Internal;
    };
    Error::legacy(
        format!(
            "Could not find any entity of type \"MiNote\" matching: {{\n    \"id\": {encoded}\n}}"
        ),
        "EntityNotFoundError",
    )
}

trait StateStore {
    fn note<'a>(
        &'a self,
        id: &'a str,
    ) -> impl Future<Output = Result<Option<StateNote>, Error>> + Send + 'a;
    fn favorite<'a>(
        &'a self,
        viewer: &'a str,
        note_id: &'a str,
    ) -> impl Future<Output = Result<bool, Error>> + Send + 'a;
    fn thread<'a>(
        &'a self,
        viewer: &'a str,
        thread_id: &'a str,
    ) -> impl Future<Output = Result<bool, Error>> + Send + 'a;
}
struct RuntimeStore<'a> {
    runtime: &'a Runtime,
}
impl RuntimeStore<'_> {
    async fn counted(
        &self,
        operation: Operation,
        sql: &str,
        viewer: &str,
        target: &str,
    ) -> Result<bool, Error> {
        let rows = self
            .runtime
            .database
            .query_typed(true, sql, &[&viewer, &target])
            .await
            .map_err(|source| Error::NoteState(StateFailure::Query { operation, source }))?;
        let [row] = rows.as_slice() else {
            return Err(shape(operation, ShapeKind::QueryShape));
        };
        row.try_get(0)
            .map_err(|source| Error::NoteState(StateFailure::Row { operation, source }))
    }
}
impl StateStore for RuntimeStore<'_> {
    async fn note(&self, id: &str) -> Result<Option<StateNote>, Error> {
        let operation = Operation::Lookup;
        let rows = self
            .runtime
            .database
            .query_typed(true, &self.runtime.state_queries.note, &[&id])
            .await
            .map_err(|source| Error::NoteState(StateFailure::Query { operation, source }))?;
        let row = match rows.as_slice() {
            [] => return Ok(None),
            [row] => row,
            _ => return Err(shape(operation, ShapeKind::QueryShape)),
        };
        let id = row
            .try_get(0)
            .map_err(|source| Error::NoteState(StateFailure::Row { operation, source }))?;
        let thread_id = row
            .try_get(1)
            .map_err(|source| Error::NoteState(StateFailure::Row { operation, source }))?;
        Ok(Some(StateNote { id, thread_id }))
    }
    async fn favorite(&self, viewer: &str, note_id: &str) -> Result<bool, Error> {
        self.counted(
            Operation::Favorite,
            &self.runtime.state_queries.favorite,
            viewer,
            note_id,
        )
        .await
    }
    async fn thread(&self, viewer: &str, thread_id: &str) -> Result<bool, Error> {
        self.counted(
            Operation::Thread,
            &self.runtime.state_queries.thread,
            viewer,
            thread_id,
        )
        .await
    }
}
async fn run(
    store: &(impl StateStore + Sync),
    note_id: &str,
    viewer: &str,
) -> Result<Value, Error> {
    let note = store
        .note(note_id)
        .await?
        .ok_or_else(|| missing_note(note_id))?;
    // Keep both independently selected reads concurrent after note lookup.
    // Nonnegative actual COUNT maps exactly to source count !==0 without JSON.
    let (favorite, thread) = tokio::try_join!(
        store.favorite(viewer, &note.id),
        store.thread(viewer, note.thread_id()),
    )?;
    Ok(Value::object([
        ("isFavorited", Value::Boolean(favorite)),
        ("isMutedThread", Value::Boolean(thread)),
    ]))
}
pub(in crate::business) async fn state(
    runtime: &Runtime,
    params: &Value,
    auth: &Context,
) -> Result<Value, Error> {
    let note_id = params
        .get("noteId")
        .and_then(Value::to_utf8)
        .ok_or_else(|| shape(Operation::Lookup, ShapeKind::InvalidParameter))?;
    let viewer = auth
        .user
        .as_ref()
        .and_then(|user| user.get("id"))
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| shape(Operation::Lookup, ShapeKind::InvalidPrincipal))?;
    run(&RuntimeStore { runtime }, &note_id, viewer).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use misskey_backend_api_auth::ColumnKind;
    use serde_json::Value as Json;
    use std::sync::Mutex;

    const SOURCE: &str = include_str!("fixtures/notes-state-typed-source-oracle.json");
    struct FixtureStore {
        note: Option<StateNote>,
        favorite: bool,
        thread: bool,
        calls: Mutex<Vec<Json>>,
        note_gate: Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
        favorite_gate: Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
        thread_gate: Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
    }
    impl StateStore for FixtureStore {
        async fn note(&self, id: &str) -> Result<Option<StateNote>, Error> {
            self.calls.lock().unwrap().push(
                serde_json::json!({"operation":"fake.note.lookup.start","criteria":{"id":id}}),
            );
            let gate = self.note_gate.lock().unwrap().take();
            if let Some(gate) = gate {
                gate.await.unwrap();
            }
            if let Some(note) = &self.note {
                self.calls
                    .lock()
                    .unwrap()
                    .push(serde_json::json!({"operation":"fake.note.lookup.resolve","id":note.id}));
            }
            Ok(self.note.clone())
        }
        async fn favorite(&self, viewer: &str, id: &str) -> Result<bool, Error> {
            self.calls.lock().unwrap().push(serde_json::json!({"operation":"fake.favorite.count.start","query":{"where":{"userId":viewer,"noteId":id},"take":1}}));
            let gate = self.favorite_gate.lock().unwrap().take();
            if let Some(gate) = gate {
                gate.await.unwrap();
            }
            Ok(self.favorite)
        }
        async fn thread(&self, viewer: &str, id: &str) -> Result<bool, Error> {
            self.calls.lock().unwrap().push(serde_json::json!({"operation":"fake.thread.count.start","query":{"where":{"userId":viewer,"threadId":id},"take":1}}));
            let gate = self.thread_gate.lock().unwrap().take();
            if let Some(gate) = gate {
                gate.await.unwrap();
            }
            Ok(self.thread)
        }
    }
    fn fixture_store(case: &Json) -> FixtureStore {
        let input = &case["input"];
        FixtureStore {
            note: (input["missing"] != true).then(|| StateNote {
                id: input["storedNote"]["id"].as_str().unwrap().to_owned(),
                thread_id: input["storedNote"]["threadId"].as_str().map(str::to_owned),
            }),
            // Native COUNT<>0 is already bool. Its typed fixture inputs come
            // from actual TS output, not a second count-to-bool implementation.
            favorite: case["expected"]["isFavorited"].as_bool().unwrap_or(false),
            thread: case["expected"]["isMutedThread"].as_bool().unwrap_or(false),
            calls: Mutex::new(Vec::new()),
            note_gate: Mutex::new(None),
            favorite_gate: Mutex::new(None),
            thread_gate: Mutex::new(None),
        }
    }
    fn source_calls(trace: &Json) -> Vec<Json> {
        trace
            .as_array()
            .unwrap()
            .iter()
            .filter(|event| {
                matches!(
                    event["operation"].as_str(),
                    Some(
                        "fake.note.lookup.start"
                            | "fake.note.lookup.resolve"
                            | "fake.favorite.count.start"
                            | "fake.thread.count.start"
                    )
                )
            })
            .cloned()
            .collect()
    }
    #[tokio::test]
    async fn all_flags_threads_loaded_ids_viewers_and_unread_payload_match_actual_source() {
        let oracle: Json = serde_json::from_str(SOURCE).unwrap();
        for case in oracle["cases"].as_array().unwrap() {
            let store = fixture_store(case);
            let result = run(
                &store,
                case["input"]["requestedId"].as_str().unwrap(),
                case["input"]["viewerId"].as_str().unwrap(),
            )
            .await;
            if let Some(expected) = case.get("expected") {
                assert_eq!(
                    result.unwrap().stringify(),
                    Value::parse_json(&expected.to_string())
                        .unwrap()
                        .stringify(),
                    "{}",
                    case["name"]
                );
            } else {
                let Error::Legacy { message, code, .. } = result.unwrap_err() else {
                    panic!("source TypeORM error expected")
                };
                assert_eq!(message, case["observedError"]["message"]);
                assert_eq!(code, case["observedError"]["name"]);
            }
            assert_eq!(
                *store.calls.lock().unwrap(),
                source_calls(&case["trace"]),
                "{}",
                case["name"]
            );
        }
    }
    #[tokio::test]
    async fn note_resolution_precedes_both_count_starts_and_both_successes_are_required() {
        let oracle: Json = serde_json::from_str(SOURCE).unwrap();
        let case = oracle["cases"].as_array().unwrap().last().unwrap();
        let store = fixture_store(case);
        let (note_tx, note_rx) = tokio::sync::oneshot::channel();
        let (favorite_tx, favorite_rx) = tokio::sync::oneshot::channel();
        let (thread_tx, thread_rx) = tokio::sync::oneshot::channel();
        *store.note_gate.lock().unwrap() = Some(note_rx);
        *store.favorite_gate.lock().unwrap() = Some(favorite_rx);
        *store.thread_gate.lock().unwrap() = Some(thread_rx);
        let mut pending = Box::pin(run(
            &store,
            case["input"]["requestedId"].as_str().unwrap(),
            case["input"]["viewerId"].as_str().unwrap(),
        ));
        assert!(futures_util::poll!(&mut pending).is_pending());
        assert_eq!(
            *store.calls.lock().unwrap(),
            source_calls(&case["beforeLookupResolve"])
        );
        note_tx.send(()).unwrap();
        assert!(futures_util::poll!(&mut pending).is_pending());
        assert_eq!(
            *store.calls.lock().unwrap(),
            source_calls(&case["beforeCountsResolve"])
        );
        thread_tx.send(()).unwrap();
        assert!(futures_util::poll!(&mut pending).is_pending());
        assert_eq!(
            *store.calls.lock().unwrap(),
            source_calls(&case["afterThreadResolve"])
        );
        favorite_tx.send(()).unwrap();
        assert_eq!(
            pending.await.unwrap().stringify(),
            Value::parse_json(&case["expected"].to_string())
                .unwrap()
                .stringify()
        );
    }

    fn column(property: &str, database: &str, kind: ColumnKind) -> EntityColumn {
        EntityColumn {
            property_name: property.into(),
            database_name: database.into(),
            kind,
        }
    }
    fn layouts() -> HashMap<String, Vec<EntityColumn>> {
        HashMap::from([
            (
                "note".into(),
                vec![
                    column("id", "stored id", ColumnKind::Json),
                    column("threadId", "stored thread", ColumnKind::Json),
                    column("opaquePayload", "unused payload", ColumnKind::Json),
                    column("timestamp", "unused date", ColumnKind::Date),
                    column("list", "unused array", ColumnKind::SimpleArray),
                ],
            ),
            (
                "note_favorite".into(),
                vec![
                    column("id", "id", ColumnKind::Json),
                    column("userId", "viewer", ColumnKind::Json),
                    column("noteId", "target", ColumnKind::Json),
                ],
            ),
            (
                "note_thread_muting".into(),
                vec![
                    column("id", "id", ColumnKind::Json),
                    column("userId", "viewer", ColumnKind::Json),
                    column("threadId", "target", ColumnKind::Json),
                ],
            ),
        ])
    }
    #[test]
    fn metadata_accepts_supported_unconsumed_columns_without_projecting_or_decoding_them() {
        let queries = StateQueries::new(&layouts()).unwrap();
        for sql in [&queries.note, &queries.favorite, &queries.thread] {
            assert!(!sql.contains("unused"));
            assert!(!sql.contains("json"));
        }
        assert!(queries.note.contains("\"stored id\""));
        assert!(queries.note.contains("\"stored thread\""));
        assert!(queries.favorite.starts_with("SELECT COUNT(*) <> 0"));
        assert!(queries.thread.starts_with("SELECT COUNT(*) <> 0"));
    }
    #[test]
    fn metadata_rejects_invalid_identifiers_and_duplicates_even_when_unconsumed() {
        for invalid in ["", "bad\0column"] {
            let mut metadata = layouts();
            metadata
                .get_mut("note")
                .unwrap()
                .push(column("ignored", invalid, ColumnKind::Date));
            assert!(matches!(
                StateQueries::new(&metadata),
                Err(StateLayoutFailure::InvalidIdentifier)
            ));
        }
        let mut metadata = layouts();
        metadata
            .get_mut("note")
            .unwrap()
            .push(column("opaquePayload", "other", ColumnKind::Json));
        assert!(matches!(
            StateQueries::new(&metadata),
            Err(StateLayoutFailure::DuplicateProperty)
        ));
        let mut metadata = layouts();
        metadata
            .get_mut("note")
            .unwrap()
            .push(column("bad\0property", "safe", ColumnKind::Json));
        assert!(matches!(
            StateQueries::new(&metadata),
            Err(StateLayoutFailure::InvalidIdentifier)
        ));
    }
    #[test]
    fn metadata_requires_consumed_properties_and_kinds_and_retains_closed_kind_decoding() {
        let mut metadata = layouts();
        metadata
            .get_mut("note")
            .unwrap()
            .retain(|column| column.property_name != "threadId");
        assert!(matches!(
            StateQueries::new(&metadata),
            Err(StateLayoutFailure::MissingProperty)
        ));
        let mut metadata = layouts();
        metadata.get_mut("note").unwrap()[0].kind = ColumnKind::Date;
        assert!(matches!(
            StateQueries::new(&metadata),
            Err(StateLayoutFailure::HydrationKind)
        ));
        assert!(
            serde_json::from_str::<EntityColumn>(
                r#"{"propertyName":"ignored","databaseName":"ignored","kind":"unsupported"}"#
            )
            .is_err()
        );
    }
}
