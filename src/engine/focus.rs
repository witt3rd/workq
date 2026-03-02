//! Focus lifecycle: create working directory, run hook pipeline, read outcome.

use crate::error::{Error, Result};
use crate::faculty::FacultyMeta;
use crate::model::work::WorkItem;
use std::path::{Path, PathBuf};
use std::time::Instant;
use tokio::process::Command;
use tracing::{debug, info, warn};
use uuid::Uuid;

/// Result of running a focus pipeline.
pub enum FocusResult {
    Completed {
        outcome_data: serde_json::Value,
        duration_ms: u64,
    },
    Failed {
        phase: String,
        error: String,
        duration_ms: u64,
    },
}

/// A focus is a temporary working context for executing a work item.
pub struct Focus {
    pub id: Uuid,
    pub dir: PathBuf,
    pub work_item: WorkItem,
}

impl Focus {
    /// Create a new focus: make the directory, write work.json.
    pub async fn create(base_dir: &Path, work_item: WorkItem) -> Result<Self> {
        let id = Uuid::new_v4();
        let dir = base_dir.join(id.to_string());
        tokio::fs::create_dir_all(&dir).await?;

        let work_json = serde_json::to_string_pretty(&work_item)
            .map_err(|e| Error::Other(format!("serialize work item: {e}")))?;
        tokio::fs::write(dir.join("work.json"), work_json).await?;

        debug!(
            focus_id = %id,
            work_id = %work_item.id,
            dir = %dir.display(),
            "focus created"
        );

        Ok(Self { id, dir, work_item })
    }

    /// Run the orient → engage → consolidate pipeline.
    pub async fn run(&self, faculty: &FacultyMeta) -> FocusResult {
        let start = Instant::now();

        // Build phase list — orient and consolidate are optional
        let mut phases: Vec<(&str, &PathBuf)> = Vec::new();
        if let Some(ref orient) = faculty.orient {
            phases.push(("orient", &orient.command));
        }
        phases.push(("engage", &faculty.engage.command));
        if let Some(ref consolidate) = faculty.consolidate {
            phases.push(("consolidate", &consolidate.command));
        }

        for (phase, command) in &phases {
            let phase_start = Instant::now();
            match self.run_hook(phase, command).await {
                Ok(()) => {
                    let phase_ms = phase_start.elapsed().as_millis() as u64;
                    info!(
                        focus_id = %self.id,
                        phase,
                        duration_ms = phase_ms,
                        "phase completed"
                    );
                }
                Err(e) => {
                    let phase_ms = phase_start.elapsed().as_millis() as u64;
                    warn!(
                        focus_id = %self.id,
                        phase,
                        duration_ms = phase_ms,
                        error = %e,
                        "phase failed"
                    );
                    return FocusResult::Failed {
                        phase: phase.to_string(),
                        error: e.to_string(),
                        duration_ms: start.elapsed().as_millis() as u64,
                    };
                }
            }
        }

        // Read outcome data — prefer consolidate-out.json, fall back to engage-out.json
        let outcome_path = if self.dir.join("consolidate-out.json").exists() {
            self.dir.join("consolidate-out.json")
        } else {
            self.dir.join("engage-out.json")
        };
        match tokio::fs::read_to_string(&outcome_path).await {
            Ok(content) => match serde_json::from_str(&content) {
                Ok(data) => FocusResult::Completed {
                    outcome_data: data,
                    duration_ms: start.elapsed().as_millis() as u64,
                },
                Err(e) => FocusResult::Failed {
                    phase: "consolidate".to_string(),
                    error: format!("bad consolidate-out.json: {e}"),
                    duration_ms: start.elapsed().as_millis() as u64,
                },
            },
            Err(e) => FocusResult::Failed {
                phase: "consolidate".to_string(),
                error: format!("missing consolidate-out.json: {e}"),
                duration_ms: start.elapsed().as_millis() as u64,
            },
        }
    }

    /// Run a single hook command.
    async fn run_hook(&self, phase: &str, command: &Path) -> Result<()> {
        // Resolve relative command paths against the process CWD (project root),
        // not the focus dir. Command::new + current_dir resolves relative paths
        // after chdir, which would look in the focus dir instead.
        let abs_command = if command.is_relative() {
            std::env::current_dir()?.join(command)
        } else {
            command.to_path_buf()
        };

        debug!(
            focus_id = %self.id,
            phase,
            command = %abs_command.display(),
            "running hook"
        );

        let status = Command::new(&abs_command)
            .current_dir(&self.dir)
            .env("ANIMUS_FOCUS_DIR", &self.dir)
            .env("ANIMUS_FACULTY", &self.work_item.faculty)
            .env("ANIMUS_WORK_ID", self.work_item.id.0.to_string())
            .env("ANIMUS_PHASE", phase)
            .status()
            .await?;

        if status.success() {
            Ok(())
        } else {
            Err(Error::Other(format!(
                "{phase} hook exited with status {}",
                status.code().unwrap_or(-1)
            )))
        }
    }

    /// Remove the focus directory.
    pub async fn cleanup(&self) -> Result<()> {
        tokio::fs::remove_dir_all(&self.dir).await?;
        debug!(focus_id = %self.id, "focus cleaned up");
        Ok(())
    }
}
