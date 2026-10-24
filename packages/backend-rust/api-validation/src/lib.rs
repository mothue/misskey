//! Native validation for the API's AJV draft-07/useDefaults contract.
//!
//! Values use UTF-16 code units and ordered own properties rather than Rust
//! strings/maps. In particular, malformed surrogate pairs, missing values,
//! negative zero and JSON.parse's overflowing numbers must survive the bridge.
mod json;
mod safe_json;
mod schema;
mod value;

pub use json::ParseError;
pub use safe_json::{SafeJson, clone_json, drop_json};
pub use schema::{CastError, CompileError, Schema, ValidationError};
pub use value::{Value, utf16};
