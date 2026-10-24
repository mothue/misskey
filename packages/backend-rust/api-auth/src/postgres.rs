//! Typed entity projection and exact auth queries over the shared PG resource.
use crate::{ColumnKind, EntityColumn, Failure, SafeJson, Store};
use async_trait::async_trait;
use misskey_backend_postgres::Database;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{collections::HashMap, sync::Arc};
use tokio_postgres::types::ToSql;

pub struct PostgresAuthStore {
    database: Arc<Database>,
    layouts: HashMap<String, Vec<EntityColumn>>,
}

impl PostgresAuthStore {
    pub fn new(
        database: Arc<Database>,
        layouts: HashMap<String, Vec<EntityColumn>>,
    ) -> Result<Self, Failure> {
        for table in ["user", "access_token", "app", "role", "role_assignment"] {
            let columns = layouts.get(table).ok_or(Failure)?;
            if columns.is_empty()
                || columns.len() > 200
                || columns.iter().any(|c| {
                    c.database_name.is_empty()
                        || c.database_name.contains('\0')
                        || c.property_name.is_empty()
                        || c.property_name.contains('\0')
                })
            {
                return Err(Failure);
            }
            let mut seen = std::collections::HashSet::new();
            if columns.iter().any(|c| !seen.insert(&c.property_name)) {
                return Err(Failure);
            }
        }
        Ok(Self { database, layouts })
    }

    fn projection(&self, table: &str) -> Result<String, Failure> {
        let columns = self.layouts.get(table).ok_or(Failure)?;
        // Build one JSON object rather than row_to_json(*), which would include
        // select:false columns and lose TypeORM's declared field order. Chunks
        // stay below PostgreSQL json_build_object's variadic argument limit.
        let mut chunks = Vec::new();
        for columns in columns.chunks(40) {
            let args: Vec<_> = columns
                .iter()
                .map(|column| {
                    let reference = format!("t.{}", quote_identifier(&column.database_name));
                    let value = if column.kind == ColumnKind::Date {
                        // pg truncates the timestamp's fractional second to
                        // three digits, including dates before the Unix epoch.
                        format!("floor(extract(epoch FROM {reference}) * 1000)::double precision")
                    } else {
                        reference
                    };
                    format!("{}, {value}", quote_literal(&column.property_name))
                })
                .collect();
            chunks.push(format!("json_build_object({})::text", args.join(", ")));
        }
        if chunks.len() == 1 {
            Ok(chunks.remove(0))
        } else {
            // JSONB concatenation would sort object keys. Project each chunk as
            // a separate column; decode() appends them in declaration order.
            Ok(chunks.join(", "))
        }
    }

    async fn select(
        &self,
        table: &str,
        tail: &str,
        params: &[&(dyn ToSql + Sync)],
    ) -> Result<Vec<Value>, Failure> {
        let projection = self.projection(table)?;
        let query = format!(
            "SELECT {projection} FROM {} t {tail}",
            quote_identifier(table)
        );
        let rows = self
            .database
            .query(true, &query, params)
            .await
            .map_err(|_| Failure)?;
        let columns = self.layouts.get(table).ok_or(Failure)?;
        let records: Vec<SafeJson> = rows
            .into_iter()
            .map(|row| {
                let mut record = SafeJson::new(Value::Object(serde_json::Map::new()));
                for index in 0..row.len() {
                    let text: String = row.try_get(index).map_err(|_| Failure)?;
                    let value = decode_json(&text)?;
                    let value = SafeJson::new(value);
                    if !value.is_object() {
                        return Err(Failure);
                    }
                    let Value::Object(values) = value.into_value() else {
                        return Err(Failure);
                    };
                    record.as_object_mut().ok_or(Failure)?.extend(values);
                }
                for column in columns {
                    if column.kind == ColumnKind::SimpleArray
                        && let Some(Value::String(text)) = record.get(&column.property_name)
                    {
                        let values = if text.is_empty() {
                            Vec::new()
                        } else {
                            text.split(',').map(str::to_owned).collect()
                        };
                        record
                            .as_object_mut()
                            .ok_or(Failure)?
                            .insert(column.property_name.clone(), json!(values));
                    }
                }
                Ok(record)
            })
            .collect::<Result<_, _>>()?;
        Ok(records.into_iter().map(SafeJson::into_value).collect())
    }
}

fn decode_json(text: &str) -> Result<Value, Failure> {
    let mut decoder = serde_json::Deserializer::from_str(text);
    decoder.disable_recursion_limit();
    let value = SafeJson::new(
        Value::deserialize(serde_stacker::Deserializer::new(&mut decoder)).map_err(|_| Failure)?,
    );
    decoder.end().map_err(|_| Failure)?;
    Ok(value.into_value())
}

#[async_trait]
impl Store for PostgresAuthStore {
    async fn user_by_token(&self, token: &str) -> Result<Option<Value>, Failure> {
        Ok(self
            .select("user", "WHERE t.token = $1 LIMIT 1", &[&token])
            .await?
            .into_iter()
            .next())
    }
    async fn user_by_id(&self, user_id: &str) -> Result<Option<Value>, Failure> {
        Ok(self
            .select("user", "WHERE t.id = $1 LIMIT 1", &[&user_id])
            .await?
            .into_iter()
            .next())
    }
    async fn access_token_by_token(
        &self,
        lower: &str,
        exact: &str,
    ) -> Result<Option<Value>, Failure> {
        Ok(self
            .select(
                "access_token",
                "WHERE t.hash = $1 OR t.token = $2 LIMIT 1",
                &[&lower, &exact],
            )
            .await?
            .into_iter()
            .next())
    }
    async fn update_last_used(&self, token_id: &str, now_ms: i64) -> Result<(), Failure> {
        let now = now_ms as f64;
        self.database.query(false, "UPDATE access_token SET \"lastUsedAt\" = to_timestamp($2::double precision / 1000) WHERE id = $1", &[&token_id, &now]).await.map_err(|_| Failure)?;
        Ok(())
    }
    async fn app_by_id(&self, app_id: &str) -> Result<Option<Value>, Failure> {
        Ok(self
            .select("app", "WHERE t.id = $1 LIMIT 1", &[&app_id])
            .await?
            .into_iter()
            .next())
    }
    async fn roles(&self) -> Result<Vec<Value>, Failure> {
        self.select("role", "", &[]).await
    }
    async fn meta(&self) -> Result<Value, Failure> {
        let rows = self.database.query(false, "SELECT json_build_object('rootUserId', \"rootUserId\", 'policies', policies, 'enableIpLogging', \"enableIpLogging\", 'preservedUsernames', \"preservedUsernames\", 'ugcVisibilityForVisitor', \"ugcVisibilityForVisitor\", 'enableReactionsBuffering', \"enableReactionsBuffering\", 'showRoleBadgesOfRemoteUsers', \"showRoleBadgesOfRemoteUsers\", 'proxyRemoteFiles', \"proxyRemoteFiles\", 'iconUrl', \"iconUrl\")::text FROM meta ORDER BY id DESC LIMIT 1", &[]).await.map_err(|_| Failure)?;
        let text: String = rows
            .first()
            .ok_or(Failure)?
            .try_get(0)
            .map_err(|_| Failure)?;
        decode_json(&text)
    }
    async fn assignments(&self, user_id: &str) -> Result<Vec<Value>, Failure> {
        self.select("role_assignment", "WHERE t.\"userId\" = $1", &[&user_id])
            .await
    }
    async fn insert_ip(&self, now_ms: i64, user_id: &str, ip: &str) -> Result<(), Failure> {
        let now = now_ms as f64;
        self.database.query(false, "INSERT INTO user_ip (\"createdAt\", \"userId\", ip) VALUES (to_timestamp($1::double precision / 1000), $2, $3) ON CONFLICT DO NOTHING", &[&now, &user_id, &ip]).await.map_err(|_| Failure)?;
        Ok(())
    }
}

fn quote_identifier(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}
fn quote_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AuthConfig, AuthService};
    use misskey_backend_postgres::{PoolSettings, PostgresConfig};

    #[tokio::test]
    #[ignore = "requires explicit dedicated online-adapter PostgreSQL fixture"]
    async fn metadata_refresh_retains_reserved_usernames_from_postgres() {
        let path =
            std::env::var("MISSKEY_POSTGRES_TEST_CONFIG").expect("dedicated config required");
        let mut config: PostgresConfig =
            serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(config.targets.len(), 1);
        assert_eq!(config.targets[0].database, "online-adapter");
        // The temporary table is connection-local; no application schema/data
        // is touched. Retain one writable connection for its lifetime.
        config.targets[0].pool = PoolSettings::default();
        let primary = config.targets[0].clone();
        let database = Arc::new(Database::new(Some(config), Some(primary)));
        database.query(false,
            "CREATE TEMP TABLE meta (id text, \"rootUserId\" text, policies jsonb, \"enableIpLogging\" boolean, \"preservedUsernames\" text[], \"ugcVisibilityForVisitor\" text, \"enableReactionsBuffering\" boolean, \"showRoleBadgesOfRemoteUsers\" boolean, \"proxyRemoteFiles\" boolean, \"iconUrl\" text)",
            &[],
        ).await.unwrap();
        database.query(false,
            "INSERT INTO meta VALUES ('fixture', NULL, '{}'::jsonb, false, ARRAY['MiXeD', 'reserved-name'], 'none', true, true, false, 'https://example.test/icon.png')",
            &[],
        ).await.unwrap();
        let store = Arc::new(PostgresAuthStore {
            database: database.clone(),
            layouts: HashMap::new(),
        });
        let auth = AuthService::new(
            store,
            AuthConfig {
                id_method: "aidx".into(),
                max_file_size: 1,
                default_policies: json!({}),
                meta: json!({"preservedUsernames":["bootstrap-only"]}),
                entity_layouts: HashMap::new(),
            },
            0,
        )
        .unwrap();
        auth.invalidate_all().await.unwrap();
        assert_eq!(
            auth.meta().await.get("ugcVisibilityForVisitor"),
            Some(&json!("none"))
        );
        assert_eq!(
            auth.meta().await.get("enableReactionsBuffering"),
            Some(&json!(true))
        );
        assert_eq!(
            auth.meta().await.get("showRoleBadgesOfRemoteUsers"),
            Some(&json!(true))
        );
        assert_eq!(
            auth.meta().await.get("proxyRemoteFiles"),
            Some(&json!(false))
        );
        assert_eq!(
            auth.meta().await.get("iconUrl"),
            Some(&json!("https://example.test/icon.png"))
        );
        assert_eq!(
            auth.meta().await.get("preservedUsernames"),
            Some(&json!(["MiXeD", "reserved-name"]))
        );
        database
            .query(
                false,
                "UPDATE meta SET \"preservedUsernames\" = ARRAY['updated']",
                &[],
            )
            .await
            .unwrap();
        auth.invalidate_all().await.unwrap();
        assert_eq!(
            auth.meta().await.get("preservedUsernames"),
            Some(&json!(["updated"]))
        );
        database.shutdown().await;
    }
}
