//! Full integration test: submit work -> read from queue -> search memory.
//!
//! Exercises the complete lifecycle across all modules. Requires Postgres
//! with pgmq + pgvector extensions (see docker-compose.yml).

use animus_rs::db::Db;
use animus_rs::model::memory::{MemoryFilters, NewMemory};
use animus_rs::model::work::NewWorkItem;

async fn test_db() -> Db {
    let url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://animus:animus_dev@localhost:5432/animus_dev".to_string());
    let db = Db::connect(&url).await.unwrap();
    db.migrate().await.unwrap();
    db
}

#[tokio::test]
#[ignore] // Requires running Postgres with pgmq + pgvector
async fn full_lifecycle() {
    let db = test_db().await;
    db.create_queue("work").await.unwrap();

    // Use a unique dedup key per run so repeated test runs don't hit dedup
    let run_id = uuid::Uuid::new_v4();
    let dedup_key = format!("person=kelly-{run_id}");

    // Submit work
    let new = NewWorkItem::new("engage", "heartbeat")
        .dedup_key(&dedup_key)
        .params(serde_json::json!({"person": "kelly"}));
    let result = db.submit_work(new).await.unwrap();
    assert!(matches!(
        result,
        animus_rs::db::work::SubmitResult::Created(_)
    ));

    // Read from queue
    let msg = db.read_from_queue("work", 30).await.unwrap();
    assert!(msg.is_some(), "expected a message in the queue");

    // Archive the message
    let msg = msg.unwrap();
    db.archive_message("work", msg.msg_id).await.unwrap();

    // Store a memory
    let embedding = vec![0.1_f32; 1536];
    let mem_id = db
        .store_memory(NewMemory {
            content: "Kelly prefers morning meetings".to_string(),
            memory_type: "relational".to_string(),
            source: Some("engage".to_string()),
            metadata: serde_json::json!({"person": "kelly"}),
            embedding: embedding.clone(),
        })
        .await
        .unwrap();
    assert!(mem_id > 0);

    // Search memory
    let results = db
        .search_memory_by_vector(&embedding, 10, &MemoryFilters::default())
        .await
        .unwrap();
    assert!(!results.is_empty());
    assert_eq!(results[0].content, "Kelly prefers morning meetings");
}
