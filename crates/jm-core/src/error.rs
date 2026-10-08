//! Errors with a stable code, a message and an actionable hint.

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    General,
    Auth,
    Validation,
    NotFound,
    Network,
    Graph,
}

impl ErrorCode {
    /// Process exit code for the CLI.
    pub fn exit_code(self) -> i32 {
        match self {
            ErrorCode::General | ErrorCode::Graph => 1,
            ErrorCode::Auth => 2,
            ErrorCode::Validation => 3,
            ErrorCode::NotFound => 4,
            ErrorCode::Network => 5,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Error {
    pub code: ErrorCode,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self { code, message: message.into(), hint: None }
    }

    pub fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    pub fn general(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::General, message)
    }

    pub fn auth(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Auth, message)
    }

    pub fn validation(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Validation, message)
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::NotFound, message)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)?;
        if let Some(hint) = &self.hint {
            write!(f, " ({hint})")?;
        }
        Ok(())
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::general(e.to_string())
    }
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Error::general(format!("unexpected JSON: {e}"))
    }
}
