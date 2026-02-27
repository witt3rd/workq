use animus_rs::db::Db;
use animus_rs::model::work::NewWorkItem;
use serde_json::json;

/// Helper: connect + migrate for tests.
/// Requires DATABASE_URL env var or defaults to local dev.
async fn test_db() -> Db {
    let url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://animus:animus_dev@localhost:5432/animus_dev".to_string());
    let db = Db::connect(&url).await.unwrap();
    db.migrate().await.unwrap();
    db
}

#[tokio::test]
#[ignore] // Requires running Postgres
async fn connects_and_migrates() {
    let db = test_db().await;
    assert!(db.health_check().await.is_ok());
}

#[tokio::test]
#[ignore] // Requires running Postgres with pgmq
async fn pgmq_send_and_read() {
    let db = test_db().await;

    // Create a queue
    db.create_queue("test_work").await.unwrap();

    // Send a message
    let msg_id = db
        .send_to_queue("test_work", &json!({"task": "hello"}), 0)
        .await
        .unwrap();
    assert!(msg_id > 0);

    // Read it back (30s visibility timeout)
    let msg = db.read_from_queue("test_work", 30).await.unwrap();
    assert!(msg.is_some());
    let msg = msg.unwrap();
    assert_eq!(msg.msg_id, msg_id);

    // Archive it
    db.archive_message("test_work", msg_id).await.unwrap();

    // Queue should be empty now
    let msg = db.read_from_queue("test_work", 30).await.unwrap();
    assert!(msg.is_none());
}

#[tokio::test]
#[ignore] // Requires running Postgres with pgmq
async fn submit_work_creates_and_queues() {
    let db = test_db().await;
    db.create_queue("work").await.unwrap();

    let new = NewWorkItem::new("engage", "heartbeat")
        .dedup_key("person=kelly")
        .params(serde_json::json!({"person": "kelly"}));

    let result = db.submit_work(new).await.unwrap();
    assert!(
        matches!(result, animus_rs::db::work::SubmitResult::Created(_)),
        "expected Created, got {result:?}"
    );
}

#[tokio::test]
#[ignore] // Requires running Postgres with pgmq
async fn submit_duplicate_work_merges() {
    let db = test_db().await;
    db.create_queue("work").await.unwrap();

    let new1 = NewWorkItem::new("engage", "heartbeat").dedup_key("person=kelly");
    let result1 = db.submit_work(new1).await.unwrap();
    assert!(matches!(
        result1,
        animus_rs::db::work::SubmitResult::Created(_)
    ));

    let new2 = NewWorkItem::new("engage", "user").dedup_key("person=kelly");
    let result2 = db.submit_work(new2).await.unwrap();
    assert!(
        matches!(result2, animus_rs::db::work::SubmitResult::Merged { .. }),
        "expected Merged, got {result2:?}"
    );
}
