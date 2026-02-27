//! Smoke tests for the full observability stack.
//!
//! These tests require the Docker Compose stack running:
//! ```sh
//! docker compose up -d
//! ```
//!
//! Run with:
//! ```sh
//! cargo test --test telemetry_smoke_test -- --ignored --nocapture
//! ```

use std::sync::OnceLock;
use std::time::Duration;

use opentelemetry::KeyValue;

static TELEMETRY: OnceLock<animus_rs::telemetry::TelemetryGuard> = OnceLock::new();

fn ensure_telemetry() -> &'static animus_rs::telemetry::TelemetryGuard {
    TELEMETRY.get_or_init(|| {
        animus_rs::telemetry::init_telemetry(animus_rs::telemetry::TelemetryConfig {
            endpoint: Some("http://localhost:4317".to_string()),
            service_name: "animus-smoke-test".to_string(),
        })
        .expect("failed to init telemetry")
    })
}

/// Force-flush all providers and give backends time to ingest.
async fn flush_and_wait(guard: &animus_rs::telemetry::TelemetryGuard) {
    guard.force_flush();
    // Give batch exporters and backends time to process.
    tokio::time::sleep(Duration::from_secs(8)).await;
}

// ---------------------------------------------------------------------------
// Traces
// ---------------------------------------------------------------------------

#[test]
#[ignore]
fn smoke_traces() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let guard = ensure_telemetry();

        // Generate trace data — spans must be entered to be exported.
        {
            let span =
                animus_rs::telemetry::work::start_work_span("smoke-work", &uuid::Uuid::new_v4());
            let _enter = span.enter();
            animus_rs::telemetry::work::record_state_transition(&span, "created", "queued");

            let genai_span = animus_rs::telemetry::genai::start_chat_span(
                "claude-sonnet-4-20250514",
                "anthropic",
            );
            let _enter2 = genai_span.enter();
            animus_rs::telemetry::genai::record_token_usage(&genai_span, 100, 50);
        }

        flush_and_wait(guard).await;

        // Query Tempo for traces from our service.
        let client = reqwest::Client::new();
        let resp = client
            .get("http://localhost:3200/api/search")
            .query(&[("tags", "service.name=animus-smoke-test"), ("limit", "5")])
            .send()
            .await
            .expect("failed to query Tempo");

        assert!(
            resp.status().is_success(),
            "Tempo query failed: {}",
            resp.status()
        );

        let body: serde_json::Value = resp.json().await.expect("failed to parse Tempo response");
        let traces = body["traces"].as_array();
        assert!(
            traces.is_some_and(|t| !t.is_empty()),
            "expected traces in Tempo, got: {body}"
        );
        println!("Tempo: found {} trace(s)", traces.unwrap().len());
    });
}

// ---------------------------------------------------------------------------
// Metrics
// ---------------------------------------------------------------------------

#[test]
#[ignore]
fn smoke_metrics() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let guard = ensure_telemetry();

        // Emit metric data.
        let counter = animus_rs::telemetry::metrics::work_submitted();
        counter.add(
            1,
            &[
                KeyValue::new("work_type", "smoke"),
                KeyValue::new("result", "ok"),
            ],
        );
        counter.add(
            1,
            &[
                KeyValue::new("work_type", "smoke"),
                KeyValue::new("result", "ok"),
            ],
        );

        let histogram = animus_rs::telemetry::metrics::operation_duration_ms();
        histogram.record(42.5, &[KeyValue::new("operation", "smoke")]);

        let transitions = animus_rs::telemetry::metrics::work_state_transitions();
        transitions.add(
            1,
            &[
                KeyValue::new("from", "created"),
                KeyValue::new("to", "queued"),
            ],
        );

        flush_and_wait(guard).await;

        // Query Prometheus for our metric.
        let client = reqwest::Client::new();
        let resp = client
            .get("http://localhost:9090/api/v1/query")
            .query(&[("query", "animus_work_submitted_total")])
            .send()
            .await
            .expect("failed to query Prometheus");

        assert!(
            resp.status().is_success(),
            "Prometheus query failed: {}",
            resp.status()
        );

        let body: serde_json::Value = resp
            .json()
            .await
            .expect("failed to parse Prometheus response");
        let results = body["data"]["result"].as_array();
        assert!(
            results.is_some_and(|r| !r.is_empty()),
            "expected metric results in Prometheus, got: {body}"
        );
        println!(
            "Prometheus: found {} series for animus_work_submitted_total",
            results.unwrap().len()
        );
    });
}

// ---------------------------------------------------------------------------
// Logs
// ---------------------------------------------------------------------------

#[test]
#[ignore]
fn smoke_logs() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let guard = ensure_telemetry();

        // Emit log data via tracing macros (bridged to OTel logs).
        tracing::info!(component = "smoke-test", "smoke test info log");
        tracing::warn!(component = "smoke-test", "smoke test warning log");

        flush_and_wait(guard).await;

        // Query Loki for logs from our service.
        let client = reqwest::Client::new();
        let resp = client
            .get("http://localhost:3100/loki/api/v1/query_range")
            .query(&[
                ("query", r#"{service_name="animus-smoke-test"}"#),
                ("limit", "10"),
            ])
            .send()
            .await
            .expect("failed to query Loki");

        assert!(
            resp.status().is_success(),
            "Loki query failed: {}",
            resp.status()
        );

        let body: serde_json::Value = resp.json().await.expect("failed to parse Loki response");
        let streams = body["data"]["result"].as_array();
        assert!(
            streams.is_some_and(|s| !s.is_empty()),
            "expected log streams in Loki, got: {body}"
        );
        println!("Loki: found {} stream(s)", streams.unwrap().len());
    });
}

// ---------------------------------------------------------------------------
// Full lifecycle
// ---------------------------------------------------------------------------

#[test]
#[ignore]
fn smoke_full_lifecycle() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let guard = ensure_telemetry();

        // Simulate a full work lifecycle generating all signal types.
        let work_id = uuid::Uuid::new_v4();

        // Traces: work + GenAI spans — enter spans so they are exported.
        {
            let work_span = animus_rs::telemetry::work::start_work_span("full-lifecycle", &work_id);
            let _work_enter = work_span.enter();
            animus_rs::telemetry::work::record_state_transition(&work_span, "created", "queued");
            animus_rs::telemetry::work::record_state_transition(&work_span, "queued", "claimed");
            animus_rs::telemetry::work::record_state_transition(&work_span, "claimed", "running");

            {
                let genai_span = animus_rs::telemetry::genai::start_chat_span(
                    "claude-sonnet-4-20250514",
                    "anthropic",
                );
                let _genai_enter = genai_span.enter();
                animus_rs::telemetry::genai::record_token_usage(&genai_span, 500, 200);
            }

            animus_rs::telemetry::work::record_state_transition(&work_span, "running", "completed");
        }

        // Metrics: counters + histogram
        let submitted = animus_rs::telemetry::metrics::work_submitted();
        submitted.add(
            1,
            &[
                KeyValue::new("work_type", "full-lifecycle"),
                KeyValue::new("result", "ok"),
            ],
        );

        let transitions = animus_rs::telemetry::metrics::work_state_transitions();
        for (from, to) in [
            ("created", "queued"),
            ("queued", "claimed"),
            ("claimed", "running"),
            ("running", "completed"),
        ] {
            transitions.add(1, &[KeyValue::new("from", from), KeyValue::new("to", to)]);
        }

        let queue_ops = animus_rs::telemetry::metrics::queue_operations();
        queue_ops.add(
            1,
            &[
                KeyValue::new("queue", "default"),
                KeyValue::new("operation", "send"),
            ],
        );
        queue_ops.add(
            1,
            &[
                KeyValue::new("queue", "default"),
                KeyValue::new("operation", "read"),
            ],
        );

        let memory_ops = animus_rs::telemetry::metrics::memory_operations();
        memory_ops.add(1, &[KeyValue::new("operation", "store")]);
        memory_ops.add(1, &[KeyValue::new("operation", "search")]);

        let duration = animus_rs::telemetry::metrics::operation_duration_ms();
        duration.record(150.0, &[KeyValue::new("operation", "work.execute")]);
        duration.record(25.0, &[KeyValue::new("operation", "memory.store")]);

        let tokens = animus_rs::telemetry::metrics::llm_tokens();
        tokens.add(
            500,
            &[
                KeyValue::new("model", "claude-sonnet-4-20250514"),
                KeyValue::new("provider", "anthropic"),
                KeyValue::new("direction", "input"),
            ],
        );
        tokens.add(
            200,
            &[
                KeyValue::new("model", "claude-sonnet-4-20250514"),
                KeyValue::new("provider", "anthropic"),
                KeyValue::new("direction", "output"),
            ],
        );

        // Logs: various levels
        tracing::info!(work_id = %work_id, work_type = "full-lifecycle", "work item submitted");
        tracing::info!(work_id = %work_id, "state transition: created -> completed");
        tracing::warn!(work_id = %work_id, "simulated warning during lifecycle");

        flush_and_wait(guard).await;

        // Verify all three backends have data.
        let client = reqwest::Client::new();

        // Tempo
        let resp = client
            .get("http://localhost:3200/api/search")
            .query(&[("tags", "service.name=animus-smoke-test"), ("limit", "5")])
            .send()
            .await
            .expect("failed to query Tempo");
        let body: serde_json::Value = resp.json().await.unwrap();
        let trace_count = body["traces"].as_array().map_or(0, |t| t.len());
        println!("Full lifecycle — Tempo: {trace_count} trace(s)");
        assert!(trace_count > 0, "expected traces in Tempo");

        // Prometheus
        let resp = client
            .get("http://localhost:9090/api/v1/query")
            .query(&[("query", "animus_work_submitted_total")])
            .send()
            .await
            .expect("failed to query Prometheus");
        let body: serde_json::Value = resp.json().await.unwrap();
        let metric_count = body["data"]["result"].as_array().map_or(0, |r| r.len());
        println!("Full lifecycle — Prometheus: {metric_count} series");
        assert!(metric_count > 0, "expected metrics in Prometheus");

        // Loki
        let resp = client
            .get("http://localhost:3100/loki/api/v1/query_range")
            .query(&[
                ("query", r#"{service_name="animus-smoke-test"}"#),
                ("limit", "10"),
            ])
            .send()
            .await
            .expect("failed to query Loki");
        let body: serde_json::Value = resp.json().await.unwrap();
        let log_count = body["data"]["result"].as_array().map_or(0, |s| s.len());
        println!("Full lifecycle — Loki: {log_count} stream(s)");
        assert!(log_count > 0, "expected logs in Loki");

        println!("Full lifecycle smoke test passed!");
    });
}
