//! Qualquer API compatível com OpenAI Chat Completions:
//! OpenAI, xAI (Grok), Google Gemini (endpoint compatível), OpenRouter,
//! Groq, Ollama/LM Studio locais, etc.

use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};

use crate::{backoff, http, retryable, ChatSession, ToolCall, ToolResult, ToolSpec, Turn};

#[derive(Clone, Debug)]
pub struct OpenAiConfig {
    pub base_url: String,
    pub api_key: Option<String>,
    pub model: String,
}

pub struct OpenAiSession {
    http: reqwest::Client,
    cfg: OpenAiConfig,
    tools: Vec<Value>,
    messages: Vec<Value>,
}

impl OpenAiSession {
    pub fn new(cfg: OpenAiConfig, system: String, tools: &[ToolSpec]) -> Self {
        let tools = tools
            .iter()
            .map(|t| {
                json!({ "type": "function", "function": { "name": t.name, "description": t.description, "parameters": t.schema } })
            })
            .collect();
        Self { http: http(), cfg, tools, messages: vec![json!({ "role": "system", "content": system })] }
    }

    async fn request(&mut self) -> Result<Turn> {
        let body = json!({
            "model": self.cfg.model,
            "messages": self.messages,
            "tools": self.tools,
            "tool_choice": "auto",
        });
        let url = format!("{}/chat/completions", self.cfg.base_url.trim_end_matches('/'));
        let mut attempt = 0;
        let v: Value = loop {
            let mut req = self.http.post(&url).json(&body);
            if let Some(k) = self.cfg.api_key.as_deref().filter(|k| !k.is_empty()) {
                req = req.bearer_auth(k);
            }
            match req.send().await {
                Ok(r) if r.status().is_success() => break r.json().await?,
                Ok(r) if retryable(r.status().as_u16()) && attempt < 4 => {
                    tokio::time::sleep(backoff(attempt, r.headers().get("retry-after"))).await;
                }
                Ok(r) => {
                    let status = r.status();
                    let text = r.text().await.unwrap_or_default();
                    let msg = serde_json::from_str::<Value>(&text)
                        .ok()
                        .and_then(|v| v["error"]["message"].as_str().map(String::from))
                        .unwrap_or_else(|| text.chars().take(300).collect());
                    bail!("API respondeu {status}: {msg}");
                }
                Err(e) if attempt < 3 && (e.is_connect() || e.is_timeout()) => {
                    tokio::time::sleep(backoff(attempt, None)).await;
                }
                Err(e) => return Err(anyhow!("falha de rede com {}: {e}", self.cfg.base_url)),
            }
            attempt += 1;
        };

        let msg = v["choices"][0]["message"].clone();
        if msg.is_null() {
            bail!("resposta sem 'choices': {}", v.to_string().chars().take(300).collect::<String>());
        }
        let mut assistant = json!({ "role": "assistant", "content": msg["content"].clone() });
        if msg["tool_calls"].is_array() {
            assistant["tool_calls"] = msg["tool_calls"].clone();
        }
        self.messages.push(assistant);

        let mut turn = Turn {
            text: msg["content"].as_str().unwrap_or_default().to_string(),
            input_tokens: v["usage"]["prompt_tokens"].as_u64().unwrap_or(0),
            output_tokens: v["usage"]["completion_tokens"].as_u64().unwrap_or(0),
            ..Default::default()
        };
        for tc in msg["tool_calls"].as_array().into_iter().flatten() {
            let args = tc["function"]["arguments"].as_str().unwrap_or("{}");
            turn.tool_calls.push(ToolCall {
                id: tc["id"].as_str().unwrap_or_default().to_string(),
                name: tc["function"]["name"].as_str().unwrap_or_default().to_string(),
                input: serde_json::from_str(args).unwrap_or_else(|_| json!({})),
            });
        }
        Ok(turn)
    }
}

#[async_trait::async_trait]
impl ChatSession for OpenAiSession {
    async fn user(&mut self, text: String) -> Result<Turn> {
        self.messages.push(json!({ "role": "user", "content": text }));
        self.request().await
    }

    async fn tool_results(&mut self, results: Vec<ToolResult>) -> Result<Turn> {
        for r in results {
            let content = if r.is_error { format!("ERRO: {}", r.content) } else { r.content };
            self.messages.push(json!({ "role": "tool", "tool_call_id": r.id, "content": content }));
        }
        self.request().await
    }
}
