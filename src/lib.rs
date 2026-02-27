//! # animus-rs
//!
//! Postgres-backed data layer for the Animus v2 AI persistence engine.
//!
//! Provides work queues (pgmq), semantic memory (pgvector via rig-postgres),
//! LLM abstraction (rig-core), and OpenTelemetry observability.

pub mod config;
pub mod db;
pub mod error;
pub mod llm;
pub mod memory;
pub mod model;
pub mod telemetry;
