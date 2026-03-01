---
status: active
milestone: M3
spec: PLAN.md § Milestone 3
code: null
---

# LLM Abstraction Design

*Thin, provider-specific HTTP clients for LLM completion calls. No framework, no agent runtime, no batteries.*

## Decision: Drop rig-core for LLM Calls

### Context

animus-rs currently uses `rig-core` (v0.31) as its LLM abstraction. The actual usage is a single function (`src/llm/mod.rs`) that creates an Anthropic client from an API key. The engage loop design (`docs/engage.md`) builds its own agentic iteration — it does not use rig-core's `Agent`, `PromptRequest`, or hook system.

### The Problem

rig-core is a batteries-included AI application framework. We use ~5% of its surface:

| What we use | What comes along |
|---|---|
| `CompletionModel::completion_request()` | Agent builder, RAG/vector stores, dynamic context stores |
| `Message`, `AssistantContent` | Embedding abstractions, eval framework |
| `ToolDefinition` | Pipeline builder, hook system |
| Anthropic provider | 15+ provider implementations |

The framework abstractions actively conflict with our design:
- rig-core's `Agent` owns the iteration loop — we need to own it for context management, ledger integration, and sandbox orchestration
- rig-core's `PromptRequest` manages chat history internally — we need to mutate it between iterations (truncate closed blocks, inject ledger context, engine nudges)
- rig-core's hook system (Terminate/Skip) is less powerful than our hook design (block/allow/modify with patches)
- rig-core's type hierarchy (`CompletionModel`, `Completion`, `Prompt`, `Chat`, `StreamingCompletion`, `StreamingPrompt`, `StreamingChat`) is complex machinery for what is ultimately an HTTP POST

Additionally:
- **Version velocity**: rig-core went from 0.1 to 0.31 with frequent breaking changes. Our LLM abstraction should be stable.
- **API landscape shift**: Anthropic has the Messages API. OpenAI has Chat Completions and the newer Responses API. Google has their own shape. We want to adapt to provider changes on our terms, not wait for a framework release.
- **Dependency weight**: rig-core brings transitive dependencies (`schemars`, `nanoid`, multiple provider SDKs) that we don't need.

### The Parallel with pgmq

pgmq provides queue semantics that are genuinely hard to implement correctly — visibility timeouts, exactly-once delivery, atomic operations. Worth the dependency.

An LLM API call is an HTTP POST with JSON in and SSE out. The complexity is in what we do *around* the call (our engage loop), not in the call itself. A thin client we own is the right abstraction level.

### What We Keep

`rig-postgres` for pgvector/embedding search — that provides real value (embedding storage, vector index, hybrid search) and would be substantial to reimplement. If `rig-postgres` requires `rig-core` as a transitive dependency, that's fine — we just stop importing `rig-core` directly for LLM calls.

---

## Design

### Module Structure

```
src/llm/
    mod.rs          — LlmClient trait, request/response types, provider factory
    types.rs        — Message, ContentBlock, ToolDefinition, Usage, StopReason
    anthropic.rs    — Anthropic Messages API implementation
    openai.rs       — OpenAI Chat Completions API implementation
    sse.rs          — Shared SSE stream parser
```

~400-500 lines total. Dependencies: `reqwest` (already in dev-dependencies, promote to dependencies), `serde`/`serde_json` (already present), `tokio` (already present), `secrecy` (already present).

### The Trait

```rust
/// Thin LLM client. One method for non-streaming, one for streaming.
/// The engage loop calls this directly — one call per iteration.
#[async_trait]
pub trait LlmClient: Send + Sync {
    /// Send a completion request, return the full response.
    async fn complete(
        &self,
        request: &CompletionRequest,
    ) -> Result<CompletionResponse, LlmError>;

    /// Send a completion request with streaming. Text deltas and partial tool
    /// JSON are emitted via `tx` as they arrive. Returns the assembled final
    /// response when the stream completes.
    async fn complete_stream(
        &self,
        request: &CompletionRequest,
        tx: &tokio::sync::mpsc::UnboundedSender<StreamEvent>,
    ) -> Result<CompletionResponse, LlmError>;
}
```

Two methods. No generics, no associated types, no phantom data, no builder pattern. The engage loop calls `complete` or `complete_stream` once per iteration and gets back exactly what it needs.

### Request

```rust
pub struct CompletionRequest {
    /// Model identifier (e.g., "claude-sonnet-4-5-20250514", "gpt-4o").
    pub model: String,

    /// System prompt. Injected as the system message / preamble.
    pub system: String,

    /// Conversation messages (user, assistant, tool_use, tool_result).
    pub messages: Vec<Message>,

    /// Tool definitions available for this call.
    pub tools: Vec<ToolDefinition>,

    /// Maximum tokens to generate.
    pub max_tokens: u32,

    /// Sampling temperature. None = provider default.
    pub temperature: Option<f64>,
}
```

### Response

```rust
pub struct CompletionResponse {
    /// Content blocks returned by the model.
    pub content: Vec<ContentBlock>,

    /// Why the model stopped generating.
    pub stop_reason: StopReason,

    /// Token usage for this call.
    pub usage: Usage,
}
```

### Types

```rust
/// A message in the conversation.
pub enum Message {
    /// System message (some providers handle this separately).
    System { content: String },

    /// User message — text, tool results, or a mix.
    User { content: Vec<UserContent> },

    /// Assistant message — text, tool calls, or a mix.
    Assistant { content: Vec<ContentBlock> },
}

/// Content in a user message.
pub enum UserContent {
    /// Plain text from the user.
    Text { text: String },

    /// Result of a tool call, matched by tool_use_id.
    ToolResult {
        tool_use_id: String,
        content: String,
        is_error: bool,
    },

    /// Image data (base64).
    Image {
        media_type: String,
        data: String,
    },
}

/// Content block in an assistant response.
pub enum ContentBlock {
    /// Text output.
    Text { text: String },

    /// Tool call request.
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
}

/// Why the model stopped.
pub enum StopReason {
    /// Normal completion — the model finished its response.
    EndTurn,
    /// The model wants to call tools.
    ToolUse,
    /// Hit the max_tokens limit.
    MaxTokens,
    /// Unknown or provider-specific reason.
    Other(String),
}

/// Token usage for a single LLM call.
pub struct Usage {
    pub input_tokens: u32,
    pub output_tokens: u32,
}

/// A tool definition sent to the LLM.
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

/// Events emitted during streaming.
pub enum StreamEvent {
    /// A chunk of text output.
    TextDelta { text: String },
    /// Partial JSON for a tool call's input (accumulated by the caller).
    ToolInputDelta { tool_use_id: String, json: String },
    /// A tool call is starting (emitted on content_block_start).
    ToolStart { tool_use_id: String, name: String },
    /// Stream complete.
    Done,
}
```

These types are our own. They map cleanly to both the Anthropic Messages API and the OpenAI Chat Completions API without being tied to either. The engage loop works with these types — never with provider-specific wire formats.

### Error Type

```rust
#[derive(Debug, thiserror::Error)]
pub enum LlmError {
    /// HTTP request failed.
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    /// LLM API returned an error response.
    #[error("API error ({status}): {message}")]
    Api { status: u16, message: String },

    /// Rate limited (429). Includes retry-after hint if provided.
    #[error("Rate limited (retry after {retry_after_secs:?}s)")]
    RateLimited { retry_after_secs: Option<u64> },

    /// JSON serialization/deserialization failed.
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    /// Stream parsing error.
    #[error("Stream error: {0}")]
    Stream(String),
}
```

`RateLimited` is a distinct variant so the caller (engage loop or a retry wrapper) can handle it specifically — exponential backoff, log the event, etc. Not buried inside a generic HTTP error.

---

## Anthropic Provider

### Wire Format

POST to `https://api.anthropic.com/v1/messages` (or configurable base URL).

```
Headers:
    x-api-key: {api_key}
    anthropic-version: 2023-06-01
    content-type: application/json

Body:
    {
        "model": "claude-sonnet-4-5-20250514",
        "max_tokens": 8192,
        "system": "You are...",
        "messages": [...],
        "tools": [...],
        "stream": false
    }
```

The Anthropic Messages API is stable and well-documented. The request/response shape maps directly to our types with minimal translation.

### Implementation Sketch

```rust
pub struct AnthropicClient {
    http: reqwest::Client,
    api_key: SecretString,
    base_url: String,
    max_retries: u32,
}

impl AnthropicClient {
    pub fn new(api_key: SecretString) -> Self {
        Self {
            http: reqwest::Client::new(),
            api_key,
            base_url: "https://api.anthropic.com".to_string(),
            max_retries: 3,
        }
    }

    pub fn with_base_url(mut self, url: String) -> Self {
        self.base_url = url;
        self
    }
}

#[async_trait]
impl LlmClient for AnthropicClient {
    async fn complete(&self, request: &CompletionRequest) -> Result<CompletionResponse, LlmError> {
        let body = self.build_request_body(request, false);
        let mut retries = 0;

        loop {
            let resp = self.http
                .post(format!("{}/v1/messages", self.base_url))
                .header("x-api-key", self.api_key.expose_secret())
                .header("anthropic-version", "2023-06-01")
                .json(&body)
                .send()
                .await?;

            match resp.status().as_u16() {
                200 => return self.parse_response(resp).await,
                429 if retries < self.max_retries => {
                    let retry_after = resp
                        .headers()
                        .get("retry-after")
                        .and_then(|v| v.to_str().ok())
                        .and_then(|v| v.parse::<u64>().ok());
                    let backoff = retry_after.unwrap_or(2u64.pow(retries));
                    tokio::time::sleep(Duration::from_secs(backoff)).await;
                    retries += 1;
                }
                429 => return Err(LlmError::RateLimited { retry_after_secs: None }),
                status => {
                    let message = resp.text().await.unwrap_or_default();
                    return Err(LlmError::Api { status, message });
                }
            }
        }
    }

    async fn complete_stream(
        &self,
        request: &CompletionRequest,
        tx: &UnboundedSender<StreamEvent>,
    ) -> Result<CompletionResponse, LlmError> {
        let body = self.build_request_body(request, true);
        let resp = self.http
            .post(format!("{}/v1/messages", self.base_url))
            .header("x-api-key", self.api_key.expose_secret())
            .header("anthropic-version", "2023-06-01")
            .json(&body)
            .send()
            .await?;

        // Parse SSE stream, emit events via tx, accumulate final response
        self.parse_stream(resp, tx).await
    }
}
```

### Type Mapping

| Our type | Anthropic wire format |
|---|---|
| `Message::User { content: [Text, ToolResult] }` | `{ "role": "user", "content": [{"type": "text", ...}, {"type": "tool_result", ...}] }` |
| `Message::Assistant { content: [Text, ToolUse] }` | `{ "role": "assistant", "content": [{"type": "text", ...}, {"type": "tool_use", ...}] }` |
| `ContentBlock::ToolUse { id, name, input }` | `{ "type": "tool_use", "id": "...", "name": "...", "input": {...} }` |
| `StopReason::ToolUse` | `"stop_reason": "tool_use"` |
| `StopReason::EndTurn` | `"stop_reason": "end_turn"` |
| `ToolDefinition { name, description, input_schema }` | `{ "name": "...", "description": "...", "input_schema": {...} }` |

The mapping is nearly 1:1. The Anthropic Messages API was designed for this exact use case.

---

## OpenAI-Compatible Provider

### Wire Format

POST to `https://api.openai.com/v1/chat/completions` (or configurable base URL for OpenRouter, Groq, Ollama, etc.).

```
Headers:
    Authorization: Bearer {api_key}
    Content-Type: application/json

Body:
    {
        "model": "gpt-4o",
        "max_tokens": 8192,
        "messages": [
            {"role": "system", "content": "You are..."},
            ...
        ],
        "tools": [...],
        "stream": false
    }
```

### Key Differences from Anthropic

| Aspect | Anthropic | OpenAI |
|---|---|---|
| System prompt | Top-level `system` field | First message with `role: "system"` |
| Tool calls in response | `content[].type == "tool_use"` | `tool_calls[].function` |
| Tool results | `content[].type == "tool_result"` with `tool_use_id` | `role: "tool"` message with `tool_call_id` |
| Stop reason | `stop_reason: "tool_use"` | `finish_reason: "tool_calls"` |
| Streaming events | `content_block_start`, `content_block_delta` | `choices[0].delta` |

The OpenAI provider handles these translation differences in `build_request_body` and `parse_response`. Our types are the canonical representation; each provider maps to/from them.

### Implementation

```rust
pub struct OpenAiClient {
    http: reqwest::Client,
    api_key: SecretString,
    base_url: String,
    max_retries: u32,
}

impl OpenAiClient {
    pub fn new(api_key: SecretString) -> Self { ... }

    /// For OpenRouter, Groq, Ollama, DeepSeek, etc.
    pub fn with_base_url(mut self, url: String) -> Self { ... }
}

#[async_trait]
impl LlmClient for OpenAiClient { ... }
```

Same trait, same return types. The engage loop doesn't know or care which provider is behind the `LlmClient`.

---

## SSE Stream Parser

Both Anthropic and OpenAI use Server-Sent Events for streaming. A shared SSE parser handles the common framing:

```rust
/// Parses an SSE byte stream into individual events.
pub struct SseParser {
    buffer: String,
}

impl SseParser {
    pub fn new() -> Self { ... }

    /// Feed bytes from the HTTP response. Returns any complete events.
    pub fn feed(&mut self, chunk: &str) -> Vec<SseEvent> { ... }
}

pub struct SseEvent {
    pub event_type: Option<String>,
    pub data: String,
}
```

Each provider implements its own event interpretation (`process_event`) that maps provider-specific SSE data payloads to our `StreamEvent` enum and accumulates the final `CompletionResponse`.

---

## Provider Factory

```rust
/// Create an LLM client from configuration.
pub fn create_client(config: &LlmConfig) -> Box<dyn LlmClient> {
    match config.provider.as_str() {
        "anthropic" => Box::new(
            AnthropicClient::new(config.api_key.clone())
                .with_base_url_opt(config.base_url.clone())
        ),
        "openai" | "openrouter" | "groq" | "ollama" | "deepseek" => Box::new(
            OpenAiClient::new(config.api_key.clone())
                .with_base_url(config.base_url.clone()
                    .unwrap_or_else(|| default_base_url(&config.provider)))
        ),
        other => panic!("Unknown LLM provider: {other}"),
    }
}
```

Configuration:

```rust
pub struct LlmConfig {
    /// Provider name: "anthropic", "openai", "openrouter", "groq", "ollama", "deepseek".
    pub provider: String,
    /// API key (secret).
    pub api_key: SecretString,
    /// Optional base URL override.
    pub base_url: Option<String>,
    /// Default model for completions.
    pub model: String,
    /// Default max tokens.
    pub max_tokens: u32,
    /// Max retries on 429.
    pub max_retries: u32,
}
```

---

## How the Engage Loop Uses This

The engage loop calls `LlmClient` directly — one call per iteration:

```rust
// Inside the engage loop
let request = CompletionRequest {
    model: faculty.engage.model.clone(),
    system: system_prompt.clone(),
    messages: messages.clone(),
    tools: tool_definitions.clone(),
    max_tokens: faculty.engage.max_tokens,
    temperature: faculty.engage.temperature,
};

let response = if let Some(ref tx) = stream_tx {
    client.complete_stream(&request, tx).await?
} else {
    client.complete(&request).await?
};

match response.stop_reason {
    StopReason::EndTurn | StopReason::MaxTokens => {
        // Extract text, persist session, return
    }
    StopReason::ToolUse => {
        // Extract tool_use blocks, execute in parallel, continue loop
        let tool_calls: Vec<_> = response.content.iter()
            .filter_map(|b| match b {
                ContentBlock::ToolUse { id, name, input } => Some((id, name, input)),
                _ => None,
            })
            .collect();

        // Parallel execution via tokio::JoinSet
        // ... (as designed in docs/engage.md § 2)
    }
    StopReason::Other(reason) => {
        // Log, extract text, return
    }
}
```

No framework intermediary. No trait hierarchies. No builder pattern. Just: build request, call API, match on stop_reason, execute tools, loop.

---

## OTel Integration

Each `LlmClient::complete` / `complete_stream` call is wrapped in a span by the engage loop (not by the client itself). The engage loop has the context — iteration number, work item, faculty — that makes the span useful:

```rust
let span = tracing::info_span!(
    "gen_ai.chat",
    gen_ai.operation.name = "chat",
    gen_ai.request.model = &request.model,
    gen_ai.provider.name = &config.provider,
    gen_ai.usage.input_tokens = tracing::field::Empty,
    gen_ai.usage.output_tokens = tracing::field::Empty,
);

let response = client.complete(&request).instrument(span.clone()).await?;

span.record("gen_ai.usage.input_tokens", response.usage.input_tokens);
span.record("gen_ai.usage.output_tokens", response.usage.output_tokens);
```

The client is deliberately dumb about tracing — it makes HTTP calls and returns data. The caller adds observability. This keeps the client testable and the tracing contextual.

---

## Migration Path

### Step 1: Add `reqwest` to dependencies (promote from dev-dependencies)

```toml
[dependencies]
reqwest = { version = "0.12", features = ["json", "stream"] }
```

### Step 2: Implement `src/llm/` module

Write `types.rs`, `sse.rs`, `anthropic.rs`, `openai.rs`, `mod.rs`. Estimated ~400-500 lines total.

### Step 3: Update `src/llm/mod.rs`

Replace the single `anthropic_client` function with `create_client` factory.

### Step 4: Update `Cargo.toml`

```toml
# LLM — thin provider clients
reqwest = { version = "0.12", features = ["json", "stream"] }

# Embeddings + Vector Search (Rig)
rig-core = "0.31"       # transitive dep for rig-postgres, no direct imports
rig-postgres = "0.1"
```

If `rig-postgres` can work without `rig-core` as a direct dependency (just transitive), remove the explicit `rig-core` line. Otherwise keep it but stop importing from it for LLM calls.

### Step 5: Update existing LLM usage

`src/llm/mod.rs` currently exports `anthropic_client()` which returns a rig `Client`. All callers switch to `create_client()` which returns `Box<dyn LlmClient>`.

---

## What We're Not Building

- **Agent runtime** — the engage loop (`docs/engage.md`) is the agent runtime. The LLM client is just the "call the API" step within it.
- **Tool execution** — the engage loop handles parallel tool execution via `tokio::JoinSet`. The LLM client doesn't know tools exist beyond passing their definitions.
- **Context management** — bounded sub-contexts, compaction, ledger injection are all in the engage loop. The LLM client receives the final `messages` vec and sends it verbatim.
- **Retry orchestration beyond 429** — the client retries on rate limits. All other error handling (model fallback, circuit breaking, etc.) is the engage loop's responsibility.
- **Embedding models** — `rig-postgres` handles this. The LLM client is for completion calls only.

## Open Questions

- **Cache control headers.** Anthropic supports `anthropic-beta: prompt-caching-2024-07-31` for prompt caching. Should the client support this? Probably yes — it's a header flag, not a structural change. The `CompletionRequest` could gain an optional `cache_control` field, or caching could be configured at the client level.

- **Extended thinking.** Anthropic's extended thinking (`anthropic-beta: extended-thinking-2025-01-24`) returns `thinking` content blocks alongside text and tool_use. Should `ContentBlock` include a `Thinking` variant? Probably not initially — thinking blocks can be treated as text or filtered. Add when needed.

- **Response metadata.** Anthropic returns a `model` field in the response (the actual model used, which may differ from the requested model). Should `CompletionResponse` include this? Useful for logging. Low cost to add.

- **Batch API.** Anthropic and OpenAI both support batch completion APIs for async, lower-cost processing. This could be useful for the consolidate phase (processing many ledger entries). Not needed now, but the `LlmClient` trait could be extended with a `complete_batch` method later.

- **Token counting.** Should the client provide a token counting method (estimate tokens for a set of messages before sending)? Useful for the engage loop's context pressure detection. But token counting is model-specific and approximate. Simpler to use character-based estimation (chars / 4) in the engage loop and let the actual usage come back in the response. Revisit if precision matters.
