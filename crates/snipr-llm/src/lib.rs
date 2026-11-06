//! Interface única de "sessão de chat com ferramentas".
//!
//! Cada provedor guarda o histórico no formato nativo dele (isso importa: a
//! API da Anthropic exige que blocos de raciocínio voltem intactos), e o
//! loop do agente só enxerga `Turn`, `ToolCall` e `ToolResult`.
//! Para adicionar um provedor novo basta implementar `ChatSession`.

pub mod anthropic;
pub mod openai;

use std::time::Duration;

use serde::Serialize;
use serde_json::Value;

#[derive(Clone, Debug, Serialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub schema: Value,
}

#[derive(Clone, Debug)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub input: Value,
}

#[derive(Clone, Debug)]
pub struct ToolResult {
    pub id: String,
    pub content: String,
    pub is_error: bool,
}

#[derive(Debug, Default)]
pub struct Turn {
    pub text: String,
    pub tool_calls: Vec<ToolCall>,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

#[async_trait::async_trait]
pub trait ChatSession: Send {
    async fn user(&mut self, text: String) -> anyhow::Result<Turn>;
    async fn tool_results(&mut self, results: Vec<ToolResult>) -> anyhow::Result<Turn>;
}

pub(crate) fn http() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(600))
        .connect_timeout(Duration::from_secs(20))
        .build()
        .expect("cliente http")
}

/// Status em que vale tentar de novo (limite de taxa, sobrecarga, erro do servidor).
pub(crate) fn retryable(status: u16) -> bool {
    matches!(status, 408 | 409 | 429 | 500 | 502 | 503 | 504 | 529)
}

pub(crate) fn backoff(attempt: u32, retry_after: Option<&reqwest::header::HeaderValue>) -> Duration {
    if let Some(secs) = retry_after.and_then(|v| v.to_str().ok()).and_then(|s| s.parse::<u64>().ok()) {
        return Duration::from_secs(secs.min(60));
    }
    Duration::from_millis(1000 * 2u64.pow(attempt).min(30))
}
