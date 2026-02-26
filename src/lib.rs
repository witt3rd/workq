//! # workq
//!
//! A work-tracking engine. Not a message bus — a system that ensures
//! work gets done exactly once.
//!
//! The engine tracks work items through their lifecycle, deduplicates
//! equivalent work, schedules execution within capacity limits, and
//! provides full observability via events and work-scoped logs.
//!
//! ## Core Concepts
//!
//! - **Work Item**: something that needs doing. Has identity, provenance,
//!   priority, and lifecycle state.
//! - **Worker**: provided by the host application. Executes work items.
//! - **Dedup**: structural (engine-provided) and semantic (host-provided)
//!   detection of duplicate work.
//! - **Events**: every state transition emits a structured event.
//! - **Logs**: workers write logs scoped to their work item.

pub mod engine;
pub mod error;
pub mod event;
pub mod model;
pub mod storage;
