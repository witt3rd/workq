//! Embedding storage, vector search, and hybrid BM25+vector search.

use crate::db::Db;
use crate::error::Result;
use crate::model::memory::*;

impl Db {
    /// Store a new memory with its embedding vector.
    pub async fn store_memory(&self, new: NewMemory) -> Result<i64> {
        let row: (i64,) = sqlx::query_as(
            "INSERT INTO memories (content, memory_type, source, metadata, embedding)
             VALUES ($1, $2, $3, $4, $5::vector)
             RETURNING id",
        )
        .bind(&new.content)
        .bind(&new.memory_type)
        .bind(&new.source)
        .bind(&new.metadata)
        .bind(format_vector(&new.embedding))
        .fetch_one(self.pool())
        .await?;
        Ok(row.0)
    }

    /// Search memories by vector similarity (cosine distance).
    pub async fn search_memory_by_vector(
        &self,
        embedding: &[f32],
        limit: i64,
        filters: &MemoryFilters,
    ) -> Result<Vec<MemoryEntry>> {
        let rows: Vec<MemoryEntryRow> = sqlx::query_as(
            "SELECT id, content, memory_type, source, metadata, created_at, updated_at
             FROM memories
             WHERE ($4::text IS NULL OR memory_type = $4)
             AND ($5::text IS NULL OR source = $5)
             AND ($6::timestamptz IS NULL OR created_at >= $6)
             ORDER BY embedding <=> $1::vector
             LIMIT $2",
        )
        .bind(format_vector(embedding))
        .bind(limit)
        .bind(0i32) // placeholder $3 not used, but keeps param numbering clean
        .bind(filters.memory_type.as_deref())
        .bind(filters.source.as_deref())
        .bind(filters.since)
        .fetch_all(self.pool())
        .await?;

        Ok(rows.into_iter().map(MemoryEntry::from).collect())
    }

    /// Hybrid search combining BM25 full-text score and vector similarity.
    pub async fn hybrid_search(
        &self,
        text: &str,
        embedding: &[f32],
        limit: i64,
        filters: &MemoryFilters,
    ) -> Result<Vec<MemoryEntry>> {
        let rows: Vec<MemoryEntryRow> = sqlx::query_as(
            "SELECT id, content, memory_type, source, metadata, created_at, updated_at
             FROM memories
             WHERE ($5::text IS NULL OR memory_type = $5)
             AND ($6::text IS NULL OR source = $6)
             AND ($7::timestamptz IS NULL OR created_at >= $7)
             ORDER BY
                (1.0 / (1e-6 + (embedding <=> $1::vector))) * 0.7
                + ts_rank(search_text, plainto_tsquery('english', $2)) * 0.3
             DESC
             LIMIT $3",
        )
        .bind(format_vector(embedding))
        .bind(text)
        .bind(limit)
        .bind(0i32) // placeholder $4 keeps param numbering clean
        .bind(filters.memory_type.as_deref())
        .bind(filters.source.as_deref())
        .bind(filters.since)
        .fetch_all(self.pool())
        .await?;

        Ok(rows.into_iter().map(MemoryEntry::from).collect())
    }
}

/// Internal row type for sqlx::FromRow.
#[derive(sqlx::FromRow)]
struct MemoryEntryRow {
    id: i64,
    content: String,
    memory_type: String,
    source: Option<String>,
    metadata: serde_json::Value,
    created_at: chrono::DateTime<chrono::Utc>,
    updated_at: chrono::DateTime<chrono::Utc>,
}

impl From<MemoryEntryRow> for MemoryEntry {
    fn from(row: MemoryEntryRow) -> Self {
        Self {
            id: row.id,
            content: row.content,
            memory_type: row.memory_type,
            source: row.source,
            metadata: row.metadata,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}

/// Format a f32 slice as a pgvector string literal: `"[0.1,0.2,0.3]"`
fn format_vector(v: &[f32]) -> String {
    let inner: Vec<String> = v.iter().map(|x| x.to_string()).collect();
    format!("[{}]", inner.join(","))
}
