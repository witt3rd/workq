//! Typed configuration from environment variables.
//!
//! Loads once at startup, fails fast if required vars are missing.
//! Sensitive values wrapped in secrecy::SecretString to prevent log leaks.

pub mod secrets;

use crate::error::{Error, Result};
use secrecy::SecretString;

#[derive(Debug)]
pub struct Config {
    pub database_url: SecretString,
    pub anthropic_api_key: SecretString,
    pub otel_endpoint: Option<String>,
    pub log_level: String,
}

impl Config {
    /// Load configuration from environment variables.
    ///
    /// In local dev, call `dotenvy::dotenv().ok()` before this.
    /// In production, systemd EnvironmentFile provides the vars.
    pub fn from_env() -> Result<Self> {
        Ok(Self {
            database_url: SecretString::from(required_var("DATABASE_URL")?),
            anthropic_api_key: SecretString::from(required_var("ANTHROPIC_API_KEY")?),
            otel_endpoint: std::env::var("OTEL_ENDPOINT").ok(),
            log_level: std::env::var("LOG_LEVEL").unwrap_or_else(|_| "info".to_string()),
        })
    }
}

fn required_var(name: &str) -> Result<String> {
    std::env::var(name)
        .map_err(|_| Error::Config(format!("required environment variable {name} is not set")))
}
