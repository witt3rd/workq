//! Work execution span helpers.
//!
//! Provides span creation and state-transition recording for work items
//! flowing through the engine.

use tracing::Span;
use uuid::Uuid;

/// Start a span for work item execution.
///
/// The `work.state` field is declared empty and can be updated via
/// [`record_state_transition`].
pub fn start_work_span(faculty: &str, work_id: &Uuid) -> Span {
    tracing::info_span!(
        "work.execute",
        "work.faculty" = faculty,
        "work.id" = %work_id,
        "work.state" = tracing::field::Empty,
    )
}

/// Record a state transition event on the current span.
///
/// Emits a tracing `info` event scoped to the given span.
pub fn record_state_transition(span: &Span, from: &str, to: &str) {
    span.in_scope(|| {
        tracing::info!(from = from, to = to, "state_transition");
    });
}
