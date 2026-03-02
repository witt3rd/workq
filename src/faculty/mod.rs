//! Faculty configuration and registry.
//!
//! A faculty is a pluggable cognitive specialization. The work item specifies
//! which faculty handles it directly — no routing table needed.

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
    #[serde(default)]
    pub concurrent: bool,
    #[serde(default)]
    pub isolation: Option<String>,
    pub orient: Option<HookConfig>,
    pub engage: HookConfig,
    pub consolidate: Option<HookConfig>,
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

/// Registry of loaded faculties, indexed by name.
pub struct FacultyRegistry {
    faculties: HashMap<String, FacultyMeta>,
}

impl FacultyRegistry {
    /// Create an empty registry with no faculties.
    pub fn empty() -> Self {
        Self {
            faculties: HashMap::new(),
        }
    }

    /// Load all `.toml` files from a directory and build the registry.
    pub fn load_from_dir(dir: &Path) -> Result<Self> {
        let mut faculties = HashMap::new();

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
                faculties.insert(meta.name.clone(), meta);
            }
        }

        Ok(Self { faculties })
    }

    /// Look up a faculty by name.
    pub fn get(&self, name: &str) -> Option<&FacultyMeta> {
        self.faculties.get(name)
    }
}
