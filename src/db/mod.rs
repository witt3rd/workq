//! Database connection pool, migrations, and health check.
//!
//! Shared Postgres connection pool used by both direct SQLx queries
//! and rig-postgres VectorStoreIndex.

pub mod pgmq;
pub mod work;

use crate::error::Result;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;

/// Database handle. Owns the connection pool shared across all modules.
pub struct Db {
    pool: PgPool,
}

impl Db {
    /// Connect to Postgres and create a connection pool.
    pub async fn connect(url: &str) -> Result<Self> {
        let pool = PgPoolOptions::new()
            .max_connections(10)
            .connect(url)
            .await?;
        Ok(Self { pool })
    }

    /// Run all pending migrations.
    pub async fn migrate(&self) -> Result<()> {
        sqlx::migrate!("./migrations")
            .run(&self.pool)
            .await
            .map_err(|e| crate::error::Error::Other(format!("migration failed: {e}")))?;
        Ok(())
    }

    /// Simple health check — run a SELECT 1.
    pub async fn health_check(&self) -> Result<()> {
        sqlx::query("SELECT 1").execute(&self.pool).await?;
        Ok(())
    }

    /// Get a reference to the connection pool (for submodules).
    #[allow(dead_code)] // used by pgmq and work submodules (Tasks 8/9)
    pub(crate) fn pool(&self) -> &PgPool {
        &self.pool
    }
}
