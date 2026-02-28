//! animus daemon — control plane that watches queues and runs foci.

use animus_rs::config::Config;
use animus_rs::db::Db;
use animus_rs::engine::{ControlConfig, ControlPlane};
use animus_rs::faculty::FacultyRegistry;
use animus_rs::telemetry::{TelemetryConfig, init_telemetry};
use secrecy::ExposeSecret;
use std::path::Path;
use std::sync::Arc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    let config = Config::from_env()?;

    let _guard = init_telemetry(TelemetryConfig {
        endpoint: config.otel_endpoint.clone(),
        service_name: "animus".to_string(),
    })?;

    let db = Db::connect(config.database_url.expose_secret()).await?;
    db.migrate().await?;
    db.create_queue("work").await?;

    let registry = FacultyRegistry::load_from_dir(Path::new("faculties"))?;

    let control = ControlPlane::new(
        Arc::new(db),
        Arc::new(registry),
        ControlConfig::default(),
        4,
    );

    // Graceful shutdown on SIGINT/SIGTERM
    let ctrl = control.clone();
    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        ctrl.shutdown();
    });

    control.run().await?;
    Ok(())
}
