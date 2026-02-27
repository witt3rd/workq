//! # animus-rs
//!
//! Postgres-backed data layer for the Animus v2 AI persistence engine.
//!
//! Provides work queues (pgmq), semantic memory (pgvector via rig-postgres),
//! LLM abstraction (rig-core), and OpenTelemetry observability.

pub mod engine;
pub mod error;
pub mod event;
pub mod model;
pub(crate) mod storage;
