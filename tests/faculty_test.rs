//! End-to-end faculty integration test.
//!
//! Requires the docker stack: `docker compose up -d`

use animus_rs::db::Db;
use animus_rs::engine::{ControlConfig, ControlPlane};
use animus_rs::faculty::FacultyRegistry;
use animus_rs::model::work::{NewWorkItem, State};
use animus_rs::telemetry::{TelemetryConfig, init_telemetry};
use std::path::Path;
use std::sync::Arc;

/// Work with no matching faculty stays queued — not dead-lettered.
/// The control plane should skip unroutable work and let the visibility
/// timeout return it to the queue for later pickup.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore] // requires docker compose up -d
async fn unroutable_work_stays_queued() {
    dotenvy::dotenv().ok();
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    let db = Db::connect(&url).await.expect("db connect");
    db.migrate().await.expect("migrate");
    db.create_queue("work").await.expect("create queue");
    let db = Arc::new(db);

    // Empty registry — no faculties at all
    let registry = FacultyRegistry::empty();

    let focus_base = std::env::temp_dir()
        .join("animus-test")
        .join(uuid::Uuid::new_v4().to_string());

    let config = ControlConfig {
        focus_base_dir: focus_base.clone(),
        visibility_timeout: 2, // short timeout so message reappears quickly
        poll_interval: std::time::Duration::from_millis(200),
    };

    let control = ControlPlane::new(Arc::clone(&db), Arc::new(registry), config, 4);

    // Start control plane
    let ctrl = control.clone();
    let handle = tokio::spawn(async move {
        ctrl.run().await.expect("control plane run");
    });

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    // Submit work with a type no faculty accepts
    let dedup = format!("test-unroutable-{}", uuid::Uuid::new_v4());
    let result = db
        .submit_work(
            NewWorkItem::new("unknown_type", "test")
                .dedup_key(&dedup)
                .params(serde_json::json!({"test": true})),
        )
        .await
        .expect("submit work");

    let work_id = match result {
        animus_rs::db::work::SubmitResult::Created(item) => item.id,
        animus_rs::db::work::SubmitResult::Merged { .. } => panic!("unexpected merge"),
    };

    // Give the control plane time to see the work and decide
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    // Work item should still be Queued — NOT Dead
    let item = db.get_work_item(work_id).await.expect("get work item");
    assert_eq!(
        item.state,
        State::Queued,
        "unroutable work should stay Queued, got {:?}",
        item.state
    );

    // Shutdown
    control.shutdown();
    let _ = tokio::time::timeout(std::time::Duration::from_secs(2), handle).await;
    let _ = tokio::fs::remove_dir_all(&focus_base).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore] // requires docker compose up -d
async fn transform_faculty_end_to_end() {
    // Initialize telemetry — sends traces/metrics/logs to OTel Collector
    dotenvy::dotenv().ok();
    let otel_endpoint =
        std::env::var("OTEL_ENDPOINT").unwrap_or_else(|_| "http://localhost:4317".to_string());
    let _guard = init_telemetry(TelemetryConfig {
        endpoint: Some(otel_endpoint),
        service_name: "animus-test".to_string(),
    })
    .expect("init telemetry");

    // Connect to test DB
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    let db = Db::connect(&url).await.expect("db connect");
    db.migrate().await.expect("migrate");
    db.create_queue("work").await.expect("create queue");

    let db = Arc::new(db);

    // Load faculties from fixtures
    let registry =
        FacultyRegistry::load_from_dir(Path::new("fixtures/faculties")).expect("load faculties");

    // Temp focus dir
    let focus_base = std::env::temp_dir()
        .join("animus-test")
        .join(uuid::Uuid::new_v4().to_string());

    let config = ControlConfig {
        focus_base_dir: focus_base.clone(),
        visibility_timeout: 30,
        poll_interval: std::time::Duration::from_millis(500),
    };

    let control = ControlPlane::new(Arc::clone(&db), Arc::new(registry), config, 4);

    // Start control plane in background
    let ctrl = control.clone();
    let handle = tokio::spawn(async move {
        ctrl.run().await.expect("control plane run");
    });

    // Give PgListener time to connect
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    // Submit a transform work item
    let dedup = format!("test-{}", uuid::Uuid::new_v4());
    let result = db
        .submit_work(
            NewWorkItem::new("transform", "test")
                .dedup_key(&dedup)
                .params(serde_json::json!({"content": "hello world"})),
        )
        .await
        .expect("submit work");

    let work_id = match result {
        animus_rs::db::work::SubmitResult::Created(item) => item.id,
        animus_rs::db::work::SubmitResult::Merged { .. } => panic!("unexpected merge"),
    };

    // Poll for completion (10s timeout)
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let item = db.get_work_item(work_id).await.expect("get work item");
        if item.state.is_terminal() {
            assert_eq!(
                item.state,
                State::Completed,
                "expected Completed, got {:?}",
                item.state
            );
            let outcome = item.outcome.expect("outcome should be present");
            assert!(outcome.success);
            let data = outcome.data.expect("outcome data");
            assert_eq!(data["verdict"], "pass");
            assert_eq!(data["result"], "dlrow olleh");
            break;
        }
        if item.state == State::Failed {
            let error_msg = item
                .outcome
                .as_ref()
                .and_then(|o| o.error.as_deref())
                .unwrap_or("unknown");
            panic!("work item failed: {error_msg}");
        }
        if tokio::time::Instant::now() > deadline {
            panic!(
                "timed out waiting for completion, current state: {:?}",
                item.state
            );
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }

    // Shutdown control plane
    control.shutdown();
    let _ = tokio::time::timeout(std::time::Duration::from_secs(2), handle).await;

    // Flush telemetry — must run off the async executor to avoid deadlock
    // with the batch exporter's tokio tasks
    let guard = _guard;
    tokio::task::spawn_blocking(move || {
        guard.force_flush();
    })
    .await
    .expect("flush telemetry");

    // Cleanup
    let _ = tokio::fs::remove_dir_all(&focus_base).await;
}
