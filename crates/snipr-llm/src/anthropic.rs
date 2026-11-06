//! Claude via API da Anthropic (Messages API, HTTP puro — Rust não tem SDK oficial).

use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};

use crate::{backoff, http, retryable, ChatSession, ToolCall, ToolResult, ToolSpec, Turn};

pub const DEFAULT_MODEL: &str = "claude-opus-5-5";
const API_VERSION: &str = "2026-10-21";

#[derive(Clone, Debug)]
pub struct AnthropicConfig {
    pub api_key: String,
    pub model: String,
    /// low | medium | high | xhigh | max (None = padrão do modelo)
    pub effort: Option<String>,
    pub base_url: String,
}

pub struct AnthropicSession {
    http: reqwest::Client,
    cfg: AnthropicConfig,
    system: String,
    tools: Vec<Value>,
    messages: Vec<Value>,
}

/// Modelos que aceitam fallback automático no servidor em caso de recusa.
fn supports_fallback(model: &str) -> bool {
    matches!(model, "claude-opus-5-5" | "claude-opus-5" | "claude-fable-5-1" | "claude-sonnet-5-5")
}

impl AnthropicSession {
    pub fn new(cfg: AnthropicConfig, system: String, tools: &[ToolSpec]) -> Self {
        let mut tools: Vec<Value> = tools
            .iter()
            .map(|t| json!({ "name": t.name, "description": t.description, "input_schema": t.schema }))
            .collect();
        // Cache do prefixo estável (ferramentas + system) entre os passos do agente.
        if let Some(last) = tools.last_mut() {
            last["cache_control"] = json!({ "type": "ephemeral" });
        }
        Self { http: http(), cfg, system, tools, messages: Vec::new() }
    }

    async fn request(&mut self) -> Result<Turn> {
        let mut body = json!({
            "model": self.cfg.model,
            "max_tokens": 16000,
            "system": [{ "type": "text", "text": self.system, "cache_control": { "type": "ephemeral" } }],
            "tools": self.tools,
            "messages": self.messages,
            // Cache automático do histórico que cresce a cada passo.
            "cache_control": { "type": "ephemeral" },
        });
        if let Some(e) = self.cfg.effort.as_deref().filter(|e| !e.is_empty()) {
            body["output_config"] = json!({ "effort": e });
        }
        let fallback = supports_fallback(&self.cfg.model);
        if fallback {
            body["fallbacks"] = json!("default");
        }

        let url = format!("{}/v1/messages", self.cfg.base_url.trim_end_matches('/'));
        let mut attempt = 0;
        let v: Value = loop {
            let mut req = self
                .http
                .post(&url)
                .header("x-api-key", &self.cfg.api_key)
                .header("anthropic-version", API_VERSION)
                .header("content-type", "application/json");
            if fallback {
                req = req.header("anthropic-beta", "server-side-fallback-2026-07-01");
            }
            let resp = req.json(&body).send().await;
            match resp {
                Ok(r) if r.status().is_success() => break r.json().await?,
                Ok(r) if retryable(r.status().as_u16()) && attempt < 4 => {
                    let wait = backoff(attempt, r.headers().get("retry-after"));
                    tracing::warn!(status = %r.status(), ?wait, "Anthropic: tentando de novo");
                    tokio::time::sleep(wait).await;
                }
                Ok(r) => {
                    let status = r.status();
                    let text = r.text().await.unwrap_or_default();
                    bail!("API Anthropic respondeu {status}: {}", extract_error(&text));
                }
                Err(e) if attempt < 3 && (e.is_connect() || e.is_timeout()) => {
                    tokio::time::sleep(backoff(attempt, None)).await;
                }
                Err(e) => return Err(anyhow!("falha de rede com a API Anthropic: {e}")),
            }
            attempt += 1;
        };

        let content = v["content"].clone();
        // O conteúdo volta exatamente como veio (inclui blocos de raciocínio).
        self.messages.push(json!({ "role": "assistant", "content": content }));

        let mut turn = Turn {
            input_tokens: v["usage"]["input_tokens"].as_u64().unwrap_or(0)
                + v["usage"]["cache_read_input_tokens"].as_u64().unwrap_or(0),
            output_tokens: v["usage"]["output_tokens"].as_u64().unwrap_or(0),
            ..Default::default()
        };
        for block in content.as_array().into_iter().flatten() {
            match block["type"].as_str() {
                Some("text") => {
                    if !turn.text.is_empty() {
                        turn.text.push('\n');
                    }
                    turn.text.push_str(block["text"].as_str().unwrap_or_default());
                }
                Some("tool_use") => turn.tool_calls.push(ToolCall {
                    id: block["id"].as_str().unwrap_or_default().to_string(),
                    name: block["name"].as_str().unwrap_or_default().to_string(),
                    input: block["input"].clone(),
                }),
                _ => {}
            }
        }
        if v["stop_reason"].as_str() == Some("refusal") {
            let why = v["stop_details"]["explanation"].as_str().unwrap_or("sem detalhes");
            bail!("o modelo recusou a tarefa ({why})");
        }
        Ok(turn)
    }
}

fn extract_error(body: &str) -> String {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| v["error"]["message"].as_str().map(String::from))
        .unwrap_or_else(|| body.chars().take(300).collect())
}

#[async_trait::async_trait]
impl ChatSession for AnthropicSession {
    async fn user(&mut self, text: String) -> Result<Turn> {
        self.messages.push(json!({ "role": "user", "content": text }));
        self.request().await
    }

    async fn tool_results(&mut self, results: Vec<ToolResult>) -> Result<Turn> {
        // Todos os resultados num único turno de usuário.
        let blocks: Vec<Value> = results
            .into_iter()
            .map(|r| json!({ "type": "tool_result", "tool_use_id": r.id, "content": r.content, "is_error": r.is_error }))
            .collect();
        self.messages.push(json!({ "role": "user", "content": blocks }));
        self.request().await
    }
}
