//! Service-layer error type.

use std::fmt;

#[derive(Debug)]
pub enum ServiceError {
    InvalidParam(String),
    NotFound(String),
    Internal(String),
}

impl fmt::Display for ServiceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidParam(msg) => write!(f, "{msg}"),
            Self::NotFound(id) => write!(f, "run not found: {id}"),
            Self::Internal(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for ServiceError {}

impl From<anyhow::Error> for ServiceError {
    fn from(e: anyhow::Error) -> Self {
        Self::Internal(e.to_string())
    }
}
