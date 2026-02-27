use animus_rs::db::Db;
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
