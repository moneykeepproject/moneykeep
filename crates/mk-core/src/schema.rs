//! JSON Schema helpers for types whose wire form differs from their Rust type.

use schemars::{Schema, SchemaGenerator, json_schema};

/// Schema for an exact decimal amount written as a JSON string, e.g. `"-1234.56"`.
///
/// Amounts are strings, never JSON numbers, so no client ever routes them through a float.
pub fn decimal_string(_: &mut SchemaGenerator) -> Schema {
    json_schema!({
        "type": "string",
        "pattern": r"^[+-]?\d+(\.\d+)?$",
        "description": "Exact decimal amount as a string, e.g. \"-1234.56\""
    })
}

/// Schema for an optional decimal amount string.
pub fn optional_decimal_string(_: &mut SchemaGenerator) -> Schema {
    json_schema!({
        "type": ["string", "null"],
        "pattern": r"^[+-]?\d+(\.\d+)?$",
        "description": "Exact decimal amount as a string, e.g. \"-1234.56\""
    })
}
