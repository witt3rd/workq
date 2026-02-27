//! Error types for animus-rs.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("work item not found: {0}")]
    NotFound(String),

    #[error("invalid state transition: {from:?} -> {to:?}")]
    InvalidTransition {
        from: crate::model::State,
        to: crate::model::State,
    },

    #[error("storage error: {0}")]
    Storage(#[from] rusqlite::Error),

    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, Error>;
