//! LLM provider setup via rig-core.
//!
//! Provides a helper function to create an Anthropic [`Client`] from a
//! [`SecretString`]-wrapped API key. The returned client can create
//! `CompletionModel` instances via rig-core's [`CompletionClient`] trait.
//!
//! # Example
//! ```no_run
//! use animus_rs::llm::anthropic_client;
//! use secrecy::SecretString;
//! use rig::client::CompletionClient;
//!
//! let key = SecretString::from("sk-ant-...");
//! let client = anthropic_client(&key).expect("failed to create Anthropic client");
//! let model = client.completion_model("claude-sonnet-4-20250514");
//! ```
//!
//! [`Client`]: rig::providers::anthropic::Client
//! [`SecretString`]: secrecy::SecretString
//! [`CompletionClient`]: rig::client::CompletionClient

use secrecy::{ExposeSecret, SecretString};

/// Create an Anthropic client from a secret API key.
///
/// The returned client can create `CompletionModel` instances via
/// [`CompletionClient::completion_model`]. Note that Anthropic does not
/// support embeddings through rig-core; use a different provider for
/// embedding models.
///
/// # Errors
/// Returns an error if the underlying HTTP client cannot be constructed.
pub fn anthropic_client(
    api_key: &SecretString,
) -> Result<rig::providers::anthropic::Client, rig::http_client::Error> {
    rig::providers::anthropic::Client::new(api_key.expose_secret())
}
