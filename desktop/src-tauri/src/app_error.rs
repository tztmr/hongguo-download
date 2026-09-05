use serde::Serialize;
use std::fmt;

#[derive(Debug, Clone, Serialize)]
pub struct AppError {
    pub code: String,
    pub message: String,
    #[serde(skip)]
    cause: Option<String>,
}

impl AppError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            cause: None,
        }
    }

    pub fn with_cause(
        code: impl Into<String>,
        message: impl Into<String>,
        cause: impl Into<String>,
    ) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            cause: Some(cause.into()),
        }
    }
}

impl fmt::Display for AppError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for AppError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        let _ = &self.cause;
        None
    }
}
