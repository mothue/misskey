/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */
//! Strict exported entity metadata and trusted SQL-template substitution.
use misskey_backend_api_auth::{ColumnKind, EntityColumn, postgres::quote_identifier};
use std::collections::{HashMap, HashSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayoutFailure {
    MissingLayout,
    InvalidIdentifier,
    UnknownProperty,
    DuplicateProperty,
    MissingProperty,
    HydrationKind,
    QueryTemplate,
}
pub(super) fn validated_layout<'a>(
    layouts: &'a HashMap<String, Vec<EntityColumn>>,
    table_name: &str,
    properties: &[(&str, ColumnKind)],
) -> Result<(&'a str, &'a [EntityColumn]), LayoutFailure> {
    let (table, columns) = layouts
        .get_key_value(table_name)
        .ok_or(LayoutFailure::MissingLayout)?;
    if table.is_empty() || table.contains('\0') {
        return Err(LayoutFailure::InvalidIdentifier);
    }
    let mut seen = HashSet::new();
    for column in columns {
        if column.database_name.is_empty() || column.database_name.contains('\0') {
            return Err(LayoutFailure::InvalidIdentifier);
        }
        let (_, kind) = properties
            .iter()
            .find(|(name, _)| *name == column.property_name)
            .ok_or(LayoutFailure::UnknownProperty)?;
        if !seen.insert(column.property_name.as_str()) {
            return Err(LayoutFailure::DuplicateProperty);
        }
        if column.kind != *kind {
            return Err(LayoutFailure::HydrationKind);
        }
    }
    if seen.len() != properties.len() {
        return Err(LayoutFailure::MissingProperty);
    }
    Ok((table.as_str(), columns.as_slice()))
}
pub(super) fn column_name(
    columns: &[EntityColumn],
    property: &str,
) -> Result<String, LayoutFailure> {
    columns
        .iter()
        .find(|column| column.property_name == property)
        .map(|column| quote_identifier(&column.database_name))
        .ok_or(LayoutFailure::MissingProperty)
}
pub(super) fn compile_query(
    template: &str,
    replacements: &[(&str, String)],
) -> Result<String, LayoutFailure> {
    // Substitute only spans of the trusted source template. Braces inside
    // quoted metadata identifiers are appended literally, never reinterpreted.
    let mut remaining = template;
    let mut compiled = String::new();
    while let Some(start) = remaining.find('{') {
        compiled.push_str(&remaining[..start]);
        let tail = &remaining[start + 1..];
        let end = tail.find('}').ok_or(LayoutFailure::QueryTemplate)?;
        let (_, replacement) = replacements
            .iter()
            .find(|(name, _)| *name == &tail[..end])
            .ok_or(LayoutFailure::QueryTemplate)?;
        compiled.push_str(replacement);
        remaining = &tail[end + 1..];
    }
    compiled.push_str(remaining);
    Ok(compiled)
}
