//! The error type shared by every validator in this module.

/// A validation failure. Each variant carries the `path` of the artifact under
/// inspection so messages are actionable.
///
/// (The field is named `path` rather than `source` because `thiserror` reserves
/// a field named `source` for the underlying `std::error::Error` cause.)
#[derive(Debug, thiserror::Error)]
pub enum ValidationError {
    /// `data` failed structural validation against the named schema. `details`
    /// is one `  <instance-path> <message>` line per failure.
    #[error("{path}: does not match the {schema} schema:\n{details}")]
    SchemaMismatch {
        path: String,
        schema: String,
        details: String,
    },

    /// Two evals share an `id` — a uniqueness constraint JSON Schema (draft-07)
    /// can't express, so it is checked by hand after structural validation.
    #[error("{path}: evals[{index}].id duplicate: {id}")]
    DuplicateId {
        path: String,
        index: usize,
        id: String,
    },

    /// The data matched the schema but could not be deserialized into the
    /// requested type — a contract drift between schema and Rust type.
    #[error("{path}: invalid value after schema validation: {message}")]
    Deserialize { path: String, message: String },
}
