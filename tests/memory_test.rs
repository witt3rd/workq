use animus_rs::db::Db;
use animus_rs::model::memory::{MemoryFilters, NewMemory};

async fn test_db() -> Db {
    let url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://animus:animus_dev@localhost:5432/animus_dev".to_string());
    let db = Db::connect(&url).await.unwrap();
    db.migrate().await.unwrap();
    db
}

#[tokio::test]
#[ignore] // Requires running Postgres with pgvector
async fn store_and_search_memory() {
    let db = test_db().await;

    let embedding = vec![0.1_f32; 1536];
    let new = NewMemory {
        content: "Kelly prefers morning meetings".to_string(),
        memory_type: "relational".to_string(),
        source: Some("engage".to_string()),
        metadata: serde_json::json!({"person": "kelly"}),
        embedding: embedding.clone(),
    };

    let id = db.store_memory(new).await.unwrap();
    assert!(id > 0);

    let results = db
        .search_memory_by_vector(&embedding, 10, &MemoryFilters::default())
        .await
        .unwrap();
    assert!(!results.is_empty());
    assert_eq!(results[0].content, "Kelly prefers morning meetings");
}

#[tokio::test]
#[ignore] // Requires running Postgres with pgvector
async fn hybrid_search_text_and_vector() {
    let db = test_db().await;

    let embedding = vec![0.1_f32; 1536];
    db.store_memory(NewMemory {
        content: "Kelly prefers morning meetings and coffee".to_string(),
        memory_type: "relational".to_string(),
        source: None,
        metadata: serde_json::json!({}),
        embedding: embedding.clone(),
    })
    .await
    .unwrap();

    let results = db
        .hybrid_search("morning coffee", &embedding, 10, &MemoryFilters::default())
        .await
        .unwrap();
    assert!(!results.is_empty());
}
