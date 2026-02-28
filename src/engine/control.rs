//! Control plane: listens for work, routes to faculties, manages focus lifecycle.

use crate::db::Db;
use crate::error::{Error, Result};
use crate::faculty::FacultyRegistry;
use crate::model::work::{Outcome, State, WorkId};
use crate::telemetry::work::{record_state_transition, start_work_span};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::Notify;
use tracing::{Instrument, error, info, warn};
use uuid::Uuid;

use super::focus::{Focus, FocusResult};

/// Configuration for the control plane.
#[derive(Debug, Clone)]
pub struct ControlConfig {
    /// Base directory for focus working directories.
    pub focus_base_dir: PathBuf,
    /// Visibility timeout (seconds) for pgmq reads.
    pub visibility_timeout: i32,
    /// Poll interval fallback when no NOTIFY arrives.
    pub poll_interval: std::time::Duration,
}

impl Default for ControlConfig {
    fn default() -> Self {
        Self {
            focus_base_dir: PathBuf::from("/tmp/animus-foci"),
            visibility_timeout: 60,
            poll_interval: std::time::Duration::from_secs(5),
        }
    }
}

/// The control plane loop: listen for work, spawn foci, retire items.
pub struct ControlPlane {
    db: Arc<Db>,
    registry: Arc<FacultyRegistry>,
    config: ControlConfig,
    shutdown: Arc<Notify>,
    active_foci: Arc<AtomicUsize>,
    max_concurrent: usize,
}

impl Clone for ControlPlane {
    fn clone(&self) -> Self {
        Self {
            db: Arc::clone(&self.db),
            registry: Arc::clone(&self.registry),
            config: self.config.clone(),
            shutdown: Arc::clone(&self.shutdown),
            active_foci: Arc::clone(&self.active_foci),
            max_concurrent: self.max_concurrent,
        }
    }
}

impl ControlPlane {
    pub fn new(
        db: Arc<Db>,
        registry: Arc<FacultyRegistry>,
        config: ControlConfig,
        max_concurrent: usize,
    ) -> Self {
        Self {
            db,
            registry,
            config,
            shutdown: Arc::new(Notify::new()),
            active_foci: Arc::new(AtomicUsize::new(0)),
            max_concurrent,
        }
    }

    /// Signal the control plane to shut down.
    pub fn shutdown(&self) {
        self.shutdown.notify_one();
    }

    /// Run the control plane loop until shutdown.
    pub async fn run(&self) -> Result<()> {
        // Ensure focus base dir exists
        tokio::fs::create_dir_all(&self.config.focus_base_dir).await?;

        // Connect PgListener for NOTIFY
        let mut listener = sqlx::postgres::PgListener::connect_with(self.db.pool()).await?;
        listener.listen("work_ready").await?;

        info!("control plane started, listening for work");

        loop {
            // Wait for: shutdown, notification, or poll timeout
            let woke = tokio::select! {
                _ = self.shutdown.notified() => {
                    info!("control plane shutting down");
                    return Ok(());
                }
                notif = listener.recv() => {
                    match notif {
                        Ok(n) => {
                            info!(work_type = n.payload(), "notified of new work");
                            true
                        }
                        Err(e) => {
                            warn!("PgListener error: {e}, falling back to poll");
                            false
                        }
                    }
                }
                _ = tokio::time::sleep(self.config.poll_interval) => {
                    false
                }
            };

            // Process available work (whether notified or polling)
            let _ = woke; // both paths lead to process_work
            if let Err(e) = self.process_work().await {
                error!("process_work error: {e}");
            }
        }
    }

    /// Try to claim and execute one work item.
    async fn process_work(&self) -> Result<()> {
        // Check capacity
        if self.active_foci.load(Ordering::Relaxed) >= self.max_concurrent {
            return Ok(());
        }

        // Read from pgmq
        let msg = self
            .db
            .read_from_queue("work", self.config.visibility_timeout)
            .await?;

        let msg = match msg {
            Some(m) => m,
            None => return Ok(()), // queue empty
        };

        // Extract work_item_id from pgmq payload
        let work_item_id = msg
            .message
            .get("work_item_id")
            .and_then(|v| v.as_str())
            .and_then(|s| Uuid::parse_str(s).ok())
            .ok_or_else(|| Error::Other("bad pgmq payload: missing work_item_id".to_string()))?;

        let work_id = WorkId(work_item_id);

        // Fetch the full work item
        let item = self.db.get_work_item(work_id).await?;

        // Create a work execution span that wraps the entire lifecycle
        let work_span = start_work_span(&item.work_type, &work_item_id);

        // Everything from routing through retirement runs inside the work span
        async {
            // Route to faculty
            let faculty = match self.registry.faculty_for_work_type(&item.work_type) {
                Some(f) => f.clone(),
                None => {
                    warn!(work_type = %item.work_type, "no faculty for work type, dead-lettering");
                    record_state_transition(&work_span, "queued", "dead");
                    self.db
                        .transition_state(work_id, State::Queued, State::Dead)
                        .await?;
                    self.db.archive_message("work", msg.msg_id).await?;
                    return Ok(());
                }
            };

            // Claim → Running
            record_state_transition(&work_span, "queued", "claimed");
            self.db
                .transition_state(work_id, State::Queued, State::Claimed)
                .await?;
            record_state_transition(&work_span, "claimed", "running");
            self.db
                .transition_state(work_id, State::Claimed, State::Running)
                .await?;

            self.active_foci.fetch_add(1, Ordering::Relaxed);

            // Create focus and run pipeline
            let focus = Focus::create(&self.config.focus_base_dir, item).await?;
            info!(
                focus_id = %focus.id,
                faculty = %faculty.name,
                "focus spawned"
            );
            let result = focus.run(&faculty).await;

            self.active_foci.fetch_sub(1, Ordering::Relaxed);

            // Retire work item based on result
            match result {
                FocusResult::Completed {
                    outcome_data,
                    duration_ms,
                } => {
                    record_state_transition(&work_span, "running", "completed");
                    info!(id = %work_id, duration_ms, "focus completed");
                    self.db
                        .complete_work(
                            work_id,
                            Outcome {
                                success: true,
                                data: Some(outcome_data),
                                error: None,
                                duration_ms,
                            },
                        )
                        .await?;
                    self.db.archive_message("work", msg.msg_id).await?;
                }
                FocusResult::Failed {
                    phase,
                    error,
                    duration_ms,
                } => {
                    record_state_transition(&work_span, "running", "failed");
                    error!(id = %work_id, phase, %error, duration_ms, "focus failed");
                    self.db
                        .fail_work(work_id, &format!("{phase}: {error}"), duration_ms)
                        .await?;
                    // Leave message in queue — visibility timeout will make it reappear
                    // for retry (v1: no recovery hook invocation)
                }
            }

            // Cleanup focus directory
            if let Err(e) = focus.cleanup().await {
                warn!(focus_id = %focus.id, "cleanup error: {e}");
            }

            Ok(())
        }
        .instrument(work_span.clone())
        .await
    }
}
