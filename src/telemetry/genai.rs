//! GenAI semantic convention span helpers for LLM operations.
//!
//! Uses OpenTelemetry GenAI semantic conventions:
//! - `gen_ai.operation.name`
//! - `gen_ai.request.model`
//! - `gen_ai.response.model`
//! - `gen_ai.provider.name`
//! - `gen_ai.usage.input_tokens`
//! - `gen_ai.usage.output_tokens`

use tracing::Span;

/// Start a span for a chat/completion operation.
///
/// Token usage fields are declared empty and can be filled later via
/// [`record_token_usage`].
pub fn start_chat_span(model: &str, provider: &str) -> Span {
    tracing::info_span!(
        "gen_ai.chat",
        "gen_ai.operation.name" = "chat",
        "gen_ai.request.model" = model,
        "gen_ai.provider.name" = provider,
        "gen_ai.usage.input_tokens" = tracing::field::Empty,
        "gen_ai.usage.output_tokens" = tracing::field::Empty,
    )
}

/// Start a span for an embedding operation.
///
/// Token usage fields are declared empty and can be filled later via
/// [`record_token_usage`].
pub fn start_embedding_span(model: &str, provider: &str) -> Span {
    tracing::info_span!(
        "gen_ai.embeddings",
        "gen_ai.operation.name" = "embeddings",
        "gen_ai.request.model" = model,
        "gen_ai.provider.name" = provider,
        "gen_ai.usage.input_tokens" = tracing::field::Empty,
        "gen_ai.usage.output_tokens" = tracing::field::Empty,
    )
}

/// Record token usage on the given span.
///
/// The span must have been created with `start_chat_span` or
/// `start_embedding_span` so that the `gen_ai.usage.*` fields exist.
pub fn record_token_usage(span: &Span, input: u64, output: u64) {
    span.record("gen_ai.usage.input_tokens", input);
    span.record("gen_ai.usage.output_tokens", output);
}
