use animus_rs::db::Db;

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
