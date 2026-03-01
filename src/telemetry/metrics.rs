//! Metric instrument factories for animus-rs.
//!
//! Uses the OTel Meter API with the globally-registered `MeterProvider`.
//! All instruments are created lazily from the `"animus-rs"` meter.

use opentelemetry::metrics::{Counter, Histogram, Meter};

/// Returns the shared meter for animus-rs instruments.
fn meter() -> Meter {
    opentelemetry::global::meter("animus-rs")
}

/// Counter: number of work items submitted.
/// Labels: `work_type`, `result` ("ok" | "duplicate" | "error").
pub fn work_submitted() -> Counter<u64> {
    meter()
        .u64_counter("animus.work.submitted")
        .with_description("Number of work items submitted")
        .build()
}

/// Counter: work item state transitions.
/// Labels: `from`, `to`.
pub fn work_state_transitions() -> Counter<u64> {
    meter()
        .u64_counter("animus.work.state_transitions")
        .with_description("Number of work item state transitions")
        .build()
}

/// Counter: queue-level operations (send, read, archive, delete).
/// Labels: `queue`, `operation`.
pub fn queue_operations() -> Counter<u64> {
    meter()
        .u64_counter("animus.queue.operations")
        .with_description("Number of queue operations")
        .build()
}

/// Counter: memory store operations (store, search, hybrid_search).
/// Labels: `operation`.
pub fn memory_operations() -> Counter<u64> {
    meter()
        .u64_counter("animus.memory.operations")
        .with_description("Number of memory store operations")
        .build()
}

/// Histogram: operation duration in milliseconds.
/// Labels: `operation`.
pub fn operation_duration_ms() -> Histogram<f64> {
    meter()
        .f64_histogram("animus.operation.duration_ms")
        .with_description("Operation duration in milliseconds")
        .with_unit("ms")
        .build()
}

/// Counter: LLM token usage.
/// Labels: `model`, `provider`, `direction` ("input" | "output").
pub fn llm_tokens() -> Counter<u64> {
    meter()
        .u64_counter("animus.llm.tokens")
        .with_description("LLM token usage")
        .build()
}

/// Counter: work items skipped because no faculty handles the work type.
/// Labels: `work_type`.
pub fn work_unroutable() -> Counter<u64> {
    meter()
        .u64_counter("animus.work.unroutable")
        .with_description("Work items with no matching faculty")
        .build()
}
