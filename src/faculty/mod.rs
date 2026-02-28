//! Faculty configuration and registry.
//!
//! A faculty is a pluggable cognitive specialization — it knows which work
//! types it handles and which external commands implement each phase of the
//! orient → engage → consolidate → recover pipeline.

use crate::error::{Error, Result};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Top-level TOML wrapper.
#[derive(Debug, Deserialize)]
struct FacultyConfig {
    faculty: FacultyMeta,
}

/// A faculty's metadata and hook configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct FacultyMeta {
    pub name: String,
    pub accepts: Vec<String>,
    pub max_concurrent: usize,
    pub orient: HookConfig,
    pub engage: HookConfig,
    pub consolidate: HookConfig,
    pub recover: RecoverConfig,
}

/// Configuration for a phase hook — just a path to an executable.
#[derive(Debug, Clone, Deserialize)]
pub struct HookConfig {
    pub command: PathBuf,
}

/// Recovery hook with retry limit.
#[derive(Debug, Clone, Deserialize)]
pub struct RecoverConfig {
    pub command: PathBuf,
    pub max_attempts: u32,
}

/// Registry of loaded faculties, indexed by name and work type.
pub struct FacultyRegistry {
    faculties: HashMap<String, FacultyMeta>,
    work_type_index: HashMap<String, String>,
}

impl FacultyRegistry {
    /// Load all `.toml` files from a directory and build the registry.
    pub fn load_from_dir(dir: &Path) -> Result<Self> {
        let mut faculties = HashMap::new();
        let mut work_type_index = HashMap::new();

        let entries = std::fs::read_dir(dir).map_err(|e| {
            Error::Config(format!("cannot read faculty dir {}: {e}", dir.display()))
        })?;

        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "toml") {
                let content = std::fs::read_to_string(&path)?;
                let config: FacultyConfig = toml::from_str(&content).map_err(|e| {
                    Error::Config(format!("bad faculty config {}: {e}", path.display()))
                })?;
                let meta = config.faculty;
                for wt in &meta.accepts {
                    work_type_index.insert(wt.clone(), meta.name.clone());
                }
                faculties.insert(meta.name.clone(), meta);
            }
        }

        Ok(Self {
            faculties,
            work_type_index,
        })
    }

    /// Look up the faculty that handles a given work type.
    pub fn faculty_for_work_type(&self, work_type: &str) -> Option<&FacultyMeta> {
        self.work_type_index
            .get(work_type)
            .and_then(|name| self.faculties.get(name))
    }
}
