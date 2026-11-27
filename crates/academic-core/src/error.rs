use thiserror::Error;

/// Unified error type returned by `academic-core` operations.
#[derive(Debug, Error)]
pub enum Error {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("configuration error: {0}")]
    Config(String),

    #[error("invalid path: {0}")]
    InvalidPath(String),

    #[error("document parse error: {0}")]
    Parse(String),

    #[error("unsupported format: {0}")]
    UnsupportedFormat(String),

    #[error("HTTP error: {0}")]
    Http(String),

    #[error("storage error: {0}")]
    Storage(String),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("TOML parse error: {0}")]
    TomlDe(#[from] toml::de::Error),

    #[error("TOML serialize error: {0}")]
    TomlSer(#[from] toml::ser::Error),

    #[error("notification error: {0}")]
    Notify(String),

    #[error("cron schedule error: {0}")]
    Cron(String),

    #[error("not found: {0}")]
    NotFound(String),

    #[error("unauthorized: {0}")]
    Unauthorized(String),
}

/// Convenient `Result` alias for `academic-core`.
pub type Result<T> = std::result::Result<T, Error>;

impl From<redb::Error> for Error {
    fn from(err: redb::Error) -> Self {
        Self::Storage(err.to_string())
    }
}

impl From<redb::DatabaseError> for Error {
    fn from(err: redb::DatabaseError) -> Self {
        Self::Storage(err.to_string())
    }
}

impl From<redb::TransactionError> for Error {
    fn from(err: redb::TransactionError) -> Self {
        Self::Storage(err.to_string())
    }
}

impl From<redb::TableError> for Error {
    fn from(err: redb::TableError) -> Self {
        Self::Storage(err.to_string())
    }
}

impl From<redb::StorageError> for Error {
    fn from(err: redb::StorageError) -> Self {
        Self::Storage(err.to_string())
    }
}

impl From<redb::CommitError> for Error {
    fn from(err: redb::CommitError) -> Self {
        Self::Storage(err.to_string())
    }
}

impl From<reqwest::Error> for Error {
    fn from(err: reqwest::Error) -> Self {
        Self::Http(err.to_string())
    }
}
