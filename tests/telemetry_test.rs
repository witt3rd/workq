//! Integration tests for telemetry initialization and span helpers.

use uuid::Uuid;

#[test]
fn telemetry_initializes_without_endpoint() {
    // Note: tracing subscriber can only be set once per process.
    // Using try_init() in the implementation avoids panics if another
    // test already initialized a subscriber.
    let config = animus_rs::telemetry::TelemetryConfig {
        endpoint: None,
        service_name: "animus-test".to_string(),
    };
    // This may return Err if a global subscriber was already set by
    // another test in this process; that is acceptable.
    let _guard = animus_rs::telemetry::init_telemetry(config);
}

#[test]
fn genai_chat_span_creates_and_records_tokens() {
    let span = animus_rs::telemetry::genai::start_chat_span("gpt-4o", "openai");
    animus_rs::telemetry::genai::record_token_usage(&span, 100, 50);
}

#[test]
fn genai_embedding_span_creates_and_records_tokens() {
    let span =
        animus_rs::telemetry::genai::start_embedding_span("text-embedding-3-small", "openai");
    animus_rs::telemetry::genai::record_token_usage(&span, 200, 0);
}

#[test]
fn work_span_creates_and_records_transition() {
    let id = Uuid::new_v4();
    let span = animus_rs::telemetry::work::start_work_span("summarize", &id);
    animus_rs::telemetry::work::record_state_transition(&span, "queued", "claimed");
}
