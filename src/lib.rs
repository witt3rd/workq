//! # animus-rs
//!
//! AI persistence engine built on Postgres.
//!
//! Covers the full stack: data plane (work queues via pgmq, semantic memory via
//! pgvector), control plane (scheduling, domain center orchestration), LLM
//! abstraction (rig-core), and observability (OpenTelemetry).

pub mod config;
pub mod db;
pub mod error;
pub mod llm;
pub mod memory;
pub mod model;
pub mod telemetry;
