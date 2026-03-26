use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("invalid input: {0}")]
    InvalidInput(String),

    #[error("not found: {0}")]
    NotFound(String),

    #[error("io error: {0}")]
    Io(String),

    #[error("git command failed: {0}")]
    Git(String),

    #[error("github resolution failed: {0}")]
    Github(String),

    #[error("json error: {0}")]
    Json(String),

    #[error("database error: {0}")]
    Db(String),

    #[error("dependency sync failed: {0}")]
    Dependency(String),

    #[error("process error: {0}")]
    Process(String),

    #[error("conflict: {0}")]
    Conflict(String),
}

impl From<std::io::Error> for AppError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value.to_string())
    }
}

impl From<rusqlite::Error> for AppError {
    fn from(value: rusqlite::Error) -> Self {
        Self::Db(value.to_string())
    }
}

impl From<reqwest::Error> for AppError {
    fn from(value: reqwest::Error) -> Self {
        Self::Github(value.to_string())
    }
}

impl From<serde_json::Error> for AppError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value.to_string())
    }
}

pub type AppResult<T> = Result<T, AppError>;
