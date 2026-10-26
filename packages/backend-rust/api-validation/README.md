# API input contract

This crate implements the validation vocabulary used by the current API
`paramDef` registry, without a JavaScript runtime. It follows the backend's
AJV configuration (`useDefaults: true`, no coercion, first error, the
`misskey:id` regular-expression format). Unsupported schema keywords and
formats fail compilation, rather than silently becoming permissive checks.
This is not a general replacement for every AJV extension.

`Schema::validate` mutates defaults at AJV's object/array rule-group stage.
Untyped rules, including `allOf` and `if`, precede the enclosing object's
defaults. `nullable` expands the accepted types but does not expand `enum` or
the declared type printed in an error. The public error is the first
`schemaPath` and `message`; later composite errors do not replace that first
error. Property schema paths use AJV's escaped JSON-pointer URI fragments.

`Schema::cast_query` implements the existing top-level GET/multipart conversion
as JSON.parse, followed separately by validation. It does not directly coerce
strings into the declared scalar type. JSON values of the wrong type therefore
parse successfully and produce the subsequent validation error. Invalid JSON
produces the separate `cannot cast to ...` error.

`Value` preserves undefined, null, binary64 numbers (including negative zero,
NaN and infinities), UTF-16 strings (including lone surrogates), arrays, and
ordered own object properties. Wire numbers are strings; wire strings and keys
are UTF-16 units. Objects enumerate array-index keys first in numeric order,
followed by other keys in insertion order. Replacing a property does not move
it. The decoder rejects unknown fields/tags, inappropriate content, invalid
units and duplicate property names.

`parse_json`/`parse_json_utf16` retain JSON.parse's numeric overflow and lone
surrogates. `stringify`/`to_json_string` follow JSON.stringify: root undefined
has no string, undefined object fields are omitted, and undefined array
members/nonfinite numbers become null. Parsing, stringification, cloning,
equality, and dropping opaque values are iterative. Transport callers must
still use an unlimited-depth serde_json deserializer with `serde_stacker`,
and `serde_stacker::Serializer`, for deeply nested tagged wire values.

The committed fixture was captured from AJV 8 in this checkout. It covers 125
cases across 23 schemas, comparing both the first error and the mutated value.
The tests also cover conversion, exact JSON representation, malformed frames,
and a 12,000-level real wire serialization/deserialization/cleanup round trip.
The broader cloud differential audit is retained outside the source tree; it
compares every current registry schema and its nested nodes. Historical
fixture success is not evidence that a later registry change has been tested.

From `packages/backend-rust`:

```sh
cargo test --locked -p misskey-api-validation
cargo clippy --locked -p misskey-api-validation --all-targets -- -D warnings
cargo fmt -p misskey-api-validation -- --check
cargo run --locked -q -p misskey-api-validation --example differential < reference.jsonl
```
