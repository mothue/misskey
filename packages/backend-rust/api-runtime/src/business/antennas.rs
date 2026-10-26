//! Complete owned antenna reads, with the original pass-through representation.
use super::{Error, iso_date, parse_id_date, rows, text};
use crate::{Runtime, response::ApiFailure};
use misskey_api_validation::Value;
use misskey_backend_api_auth::{
    ColumnKind, Context, EntityColumn,
    postgres::{quote_identifier, quote_literal},
};
use std::collections::{HashMap, HashSet};

const LIST_SQL: &str = r#"SELECT {projection} FROM {table} a WHERE a.{owner}=$1"#;
const SHOW_SQL: &str =
    r#"SELECT {projection} FROM {table} a WHERE a.{id}=$1 AND a.{owner}=$2 LIMIT 1"#;

// These are consumed entity properties, never SQL database identifiers. The
// complete selected shape is checked against TypeORM's bootstrap metadata.
const PROPERTIES: &[(&str, ColumnKind)] = &[
    ("id", ColumnKind::Json),
    ("lastUsedAt", ColumnKind::Date),
    ("userId", ColumnKind::Json),
    ("name", ColumnKind::Json),
    ("src", ColumnKind::Json),
    ("userListId", ColumnKind::Json),
    ("users", ColumnKind::Json),
    ("keywords", ColumnKind::Json),
    ("excludeKeywords", ColumnKind::Json),
    ("caseSensitive", ColumnKind::Json),
    ("excludeBots", ColumnKind::Json),
    ("withReplies", ColumnKind::Json),
    ("withFile", ColumnKind::Json),
    ("expression", ColumnKind::Json),
    ("isActive", ColumnKind::Json),
    ("localOnly", ColumnKind::Json),
    ("excludeNotesInSensitiveChannel", ColumnKind::Json),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FailureKind {
    MissingLayout,
    InvalidIdentifier,
    UnknownProperty,
    DuplicateProperty,
    MissingProperty,
    HydrationKind,
    QueryTemplate,
    InvalidPrincipal,
    MissingField,
    FieldType,
}

pub(crate) struct Queries {
    list: String,
    show: String,
}

fn validate_columns(columns: &[EntityColumn]) -> Result<(), FailureKind> {
    let mut seen = HashSet::new();
    for column in columns {
        if column.database_name.is_empty() || column.database_name.contains('\0') {
            return Err(FailureKind::InvalidIdentifier);
        }
        let (_, expected) = PROPERTIES
            .iter()
            .find(|(property, _)| *property == column.property_name)
            .ok_or(FailureKind::UnknownProperty)?;
        if !seen.insert(column.property_name.as_str()) {
            return Err(FailureKind::DuplicateProperty);
        }
        if *expected != column.kind {
            return Err(FailureKind::HydrationKind);
        }
    }
    if seen.len() != PROPERTIES.len() {
        return Err(FailureKind::MissingProperty);
    }
    Ok(())
}

struct QueryParts {
    table: String,
    projection: String,
    owner: String,
    id: String,
}
impl QueryParts {
    fn render(&self, template: &str) -> Result<String, FailureKind> {
        // Substitute source spans only: a quoted identifier containing braces
        // must not become a new template expression in a later replacement.
        let mut result = String::new();
        let mut remaining = template;
        while let Some(start) = remaining.find('{') {
            result.push_str(&remaining[..start]);
            let tail = &remaining[start + 1..];
            let end = tail.find('}').ok_or(FailureKind::QueryTemplate)?;
            result.push_str(match &tail[..end] {
                "projection" => &self.projection,
                "table" => &self.table,
                "owner" => &self.owner,
                "id" => &self.id,
                _ => return Err(FailureKind::QueryTemplate),
            });
            remaining = &tail[end + 1..];
        }
        result.push_str(remaining);
        Ok(result)
    }
}

impl Queries {
    pub(crate) fn new(layouts: &HashMap<String, Vec<EntityColumn>>) -> Result<Self, FailureKind> {
        let (table, columns) = layouts
            .get_key_value("antenna")
            .ok_or(FailureKind::MissingLayout)?;
        validate_columns(columns)?;
        let reference = |property: &str| {
            columns
                .iter()
                .find(|column| column.property_name == property)
                .map(|column| quote_identifier(&column.database_name))
                .ok_or(FailureKind::MissingProperty)
        };
        // Keep the full selected entity projection and its physical scan, like
        // findBy/findOneBy. Dates not emitted by this DTO need no JS hydration.
        let projection = format!(
            "json_build_object({})::text",
            columns
                .iter()
                .map(|column| {
                    format!(
                        "{}, a.{}",
                        quote_literal(&column.property_name),
                        quote_identifier(&column.database_name)
                    )
                })
                .collect::<Vec<_>>()
                .join(", ")
        );
        let parts = QueryParts {
            table: quote_identifier(table),
            projection,
            owner: reference("userId")?,
            id: reference("id")?,
        };
        Ok(Self {
            list: parts.render(LIST_SQL)?,
            show: parts.render(SHOW_SQL)?,
        })
    }
}

fn owner(auth: &Context) -> Result<&str, Error> {
    auth.user
        .as_ref()
        .and_then(|user| user.get("id"))
        .and_then(serde_json::Value::as_str)
        .ok_or(Error::Antenna(FailureKind::InvalidPrincipal))
}
fn field<'a>(row: &'a Value, name: &str) -> Result<&'a Value, Error> {
    row.get(name)
        .ok_or(Error::Antenna(FailureKind::MissingField))
}
fn string(row: &Value, name: &str) -> Result<Value, Error> {
    match field(row, name)? {
        value @ Value::String(_) => Ok(value.clone()),
        _ => Err(Error::Antenna(FailureKind::FieldType)),
    }
}
fn nullable_string(row: &Value, name: &str) -> Result<Value, Error> {
    match field(row, name)? {
        value @ (Value::String(_) | Value::Null) => Ok(value.clone()),
        _ => Err(Error::Antenna(FailureKind::FieldType)),
    }
}
fn boolean(row: &Value, name: &str) -> Result<Value, Error> {
    match field(row, name)? {
        value @ Value::Boolean(_) => Ok(value.clone()),
        _ => Err(Error::Antenna(FailureKind::FieldType)),
    }
}

fn invalid_date() -> Error {
    let definition = &crate::response::INVALID_DATE;
    Error::legacy(definition.message.to_owned(), definition.code)
}

fn pack(row: &Value, id_method: &str) -> Result<Value, Error> {
    // IdService.parse does not apply request-ID validation to stored IDs.
    // Preserve its successful irregular prefixes and Bun's RangeError on NaN.
    let id = text(row, "id")?;
    let millis = parse_id_date(id_method, &id).ok_or_else(invalid_date)?;
    // iso_date's only failure is a nonrepresentable JS date, whose observed
    // public class/message is the same RangeError as a failed ID parse.
    let created_at = iso_date(millis).map_err(|_date_error| invalid_date())?;
    Ok(Value::object([
        ("id", string(row, "id")?),
        ("createdAt", Value::string(&created_at)),
        ("name", string(row, "name")?),
        // The legacy packer copies jsonb/array data without nested validation.
        // Value preserves UTF-16, depth, ordering and malformed stored JSON.
        ("keywords", field(row, "keywords")?.clone()),
        ("excludeKeywords", field(row, "excludeKeywords")?.clone()),
        ("src", string(row, "src")?),
        ("userListId", nullable_string(row, "userListId")?),
        ("users", field(row, "users")?.clone()),
        ("caseSensitive", boolean(row, "caseSensitive")?),
        ("localOnly", boolean(row, "localOnly")?),
        ("excludeBots", boolean(row, "excludeBots")?),
        ("withReplies", boolean(row, "withReplies")?),
        ("withFile", boolean(row, "withFile")?),
        (
            "excludeNotesInSensitiveChannel",
            boolean(row, "excludeNotesInSensitiveChannel")?,
        ),
        ("isActive", boolean(row, "isActive")?),
        ("hasUnreadNote", Value::Boolean(false)),
        ("notify", Value::Boolean(false)),
    ]))
}

pub(super) async fn list(runtime: &Runtime, auth: &Context) -> Result<Value, Error> {
    let owner = owner(auth)?;
    let selected = rows(
        &runtime.database,
        true,
        &runtime.antenna_queries.list,
        &[&owner],
    )
    .await?;
    // No sort, cap, pagination, extra user lookup, cache or unread query.
    selected
        .iter()
        .map(|row| pack(row, &runtime.business_config.id_method))
        .collect::<Result<Vec<_>, _>>()
        .map(Value::Array)
}

pub(super) async fn show(
    runtime: &Runtime,
    params: &Value,
    auth: &Context,
) -> Result<Value, Error> {
    let owner = owner(auth)?;
    let id = text(params, "antennaId")?;
    let selected = rows(
        &runtime.database,
        true,
        &runtime.antenna_queries.show,
        &[&id, &owner],
    )
    .await?;
    // The ownership predicate runs in SQL, before interpreting any payload.
    let row = selected
        .first()
        .ok_or(Error::Fixed(ApiFailure::NoSuchAntenna))?;
    pack(row, &runtime.business_config.id_method)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn merged(base: &Value, changes: &Value) -> Value {
        let mut result = base.clone();
        let Value::Object(changes) = changes else {
            panic!("oracle change object");
        };
        for (key, value) in changes {
            let key = String::from_utf16(key).expect("ASCII fixture property");
            result.set(&key, value.clone());
        }
        result
    }

    #[test]
    fn packing_matches_actual_bun_oracle_including_legacy_dates_and_stored_json() {
        let oracle = Value::parse_json(include_str!("fixtures/antenna-packing.json")).unwrap();
        let base_input = oracle.get("baseInput").unwrap();
        let base_result = oracle.get("baseResult").unwrap();
        let Value::Array(cases) = oracle.get("cases").unwrap() else {
            panic!("oracle cases");
        };
        for case in cases {
            let method = case.get("method").unwrap().to_utf8().unwrap();
            let input = merged(base_input, case.get("inputChanges").unwrap());
            let result = pack(&input, &method);
            if let Some(expected) = case.get("error") {
                let Error::Legacy { message, code, .. } = result.expect_err("oracle date failure")
                else {
                    panic!("expected observed legacy date error");
                };
                assert_eq!(
                    Some(message),
                    expected.get("message").and_then(Value::to_utf8)
                );
                assert_eq!(
                    Some(code.to_owned()),
                    expected.get("name").and_then(Value::to_utf8)
                );
            } else {
                let expected = merged(base_result, case.get("resultChanges").unwrap());
                assert_eq!(result.unwrap(), expected, "method={method}");
            }
        }
    }

    fn fixture_layout() -> HashMap<String, Vec<EntityColumn>> {
        HashMap::from([(
            "antenna".to_owned(),
            PROPERTIES
                .iter()
                .map(|(name, kind)| EntityColumn {
                    property_name: (*name).to_owned(),
                    database_name: (*name).to_owned(),
                    kind: *kind,
                })
                .collect(),
        )])
    }

    #[test]
    fn incomplete_antenna_metadata_cannot_reach_readiness() {
        let valid = fixture_layout();
        assert!(Queries::new(&valid).is_ok());
        for (property, _) in PROPERTIES {
            let mut incomplete = valid.clone();
            incomplete
                .get_mut("antenna")
                .unwrap()
                .retain(|column| column.property_name != *property);
            assert!(
                matches!(Queries::new(&incomplete), Err(FailureKind::MissingProperty)),
                "{property}"
            );
        }
    }

    #[test]
    fn typed_principal_is_required_even_without_admission() {
        let anonymous = Context {
            user: None,
            token: None,
            access_token_reduced: false,
        };
        assert!(matches!(
            owner(&anonymous),
            Err(Error::Antenna(FailureKind::InvalidPrincipal))
        ));
        let malformed = Context {
            user: Some(serde_json::json!({"id":1})),
            token: None,
            access_token_reduced: false,
        };
        assert!(matches!(
            owner(&malformed),
            Err(Error::Antenna(FailureKind::InvalidPrincipal))
        ));
    }
}
