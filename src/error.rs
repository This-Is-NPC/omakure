use std::io;
use thiserror::Error;

/// Application error type covering all error categories.
#[derive(Debug, Error)]
pub enum AppError {
    #[error("IO error: {0}")]
    Io(#[from] io::Error),

    #[error("Schema error: {0}")]
    Schema(#[from] SchemaError),

    #[error("Script error: {0}")]
    Script(#[from] ScriptError),

    #[error("Environment error: {0}")]
    Environment(#[from] EnvironmentError),

    #[error("{0}")]
    General(String),
}

/// Errors related to schema parsing.
#[derive(Debug, Error)]
pub enum SchemaError {
    #[error("Schema block not found in script")]
    BlockNotFound,

    #[error("Schema block is empty")]
    EmptyBlock,

    #[error("Schema block line missing comment prefix at line {line}")]
    MissingCommentPrefix { line: usize },

    #[error("Invalid JSON in schema: {0}")]
    InvalidJson(#[from] serde_json::Error),

    #[error("Schema JSON object not found in output")]
    JsonNotFound,

    #[error("Invalid cron expression `{expr}`: {reason}")]
    InvalidCron { expr: String, reason: String },

    #[error("Unsupported secret field construct for `{field}`: {construct}")]
    UnsupportedSecretFieldConstruct {
        field: String,
        construct: &'static str,
    },
}

/// Errors related to script execution.
#[derive(Debug, Error)]
pub enum ScriptError {
    #[error("Unsupported script type")]
    UnsupportedType,

    #[error("{name} not found in PATH. {hint}")]
    DependencyMissing { name: String, hint: String },

    #[error("{name} found, but check failed: {message}")]
    DependencyCheckFailed { name: String, message: String },
}

/// Errors related to environment configuration.
#[derive(Debug, Error)]
pub enum EnvironmentError {
    #[error("Environment not found: {name}")]
    NotFound { name: String },

    #[error("Invalid environment name: {name}")]
    InvalidName { name: String },

    #[error("Unsafe environment path: {path}")]
    UnsafePath { path: String },

    #[error("Failed to read environment: {0}")]
    ReadFailed(String),

    #[error("Failed to write environment: {0}")]
    WriteFailed(String),
}

/// Result type alias using AppError.
pub type AppResult<T> = Result<T, AppError>;

impl From<String> for AppError {
    fn from(msg: String) -> Self {
        AppError::General(msg)
    }
}

impl From<&str> for AppError {
    fn from(msg: &str) -> Self {
        AppError::General(msg.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_app_error_display() {
        let err = AppError::Schema(SchemaError::BlockNotFound);
        assert_eq!(
            format!("{}", err),
            "Schema error: Schema block not found in script"
        );
    }

    #[test]
    fn test_script_error_display() {
        let err = ScriptError::DependencyMissing {
            name: "bash".to_string(),
            hint: "Install bash".to_string(),
        };
        assert_eq!(format!("{}", err), "bash not found in PATH. Install bash");
    }

    #[test]
    fn test_app_error_from_string() {
        let err: AppError = "something went wrong".into();
        assert_eq!(format!("{}", err), "something went wrong");
    }

    #[test]
    fn test_app_error_from_io() {
        let io_err = io::Error::new(io::ErrorKind::NotFound, "file not found");
        let err = AppError::from(io_err);
        assert!(matches!(err, AppError::Io(_)));
        assert!(format!("{}", err).contains("file not found"));
    }

    #[test]
    fn test_app_error_from_owned_string() {
        let owned: String = String::from("oops");
        let err: AppError = owned.into();
        assert!(matches!(err, AppError::General(_)));
        assert_eq!(format!("{}", err), "oops");
    }

    #[test]
    fn test_schema_error_from_serde() {
        let json_err = serde_json::from_str::<serde_json::Value>("invalid").unwrap_err();
        let err = SchemaError::from(json_err);
        assert!(matches!(err, SchemaError::InvalidJson(_)));
    }
}
