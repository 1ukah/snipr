//! Loop de agente para provedores via API (Anthropic, OpenAI-compatível).

use std::sync::Arc;

use anyhow::{bail, Result};
use snipr_llm::{ChatSession, ToolResult};
use tokio_util::sync::CancellationToken;

use crate::tools::ToolBox;

pub async fn run_chat_agent(
    mut session: Box<dyn ChatSession>,
    task: String,
    tools: Arc<ToolBox>,
    max_steps: usize,
    cancel: CancellationToken,
) -> Result<String> {
    let mut tokens = (0u64, 0u64);
    let mut turn = tokio::select! {
        r = session.user(task) => r?,
        _ = cancel.cancelled() => bail!("cancelado"),
    };
    for _ in 0..max_steps {
        tokens.0 += turn.input_tokens;
        tokens.1 += turn.output_tokens;
        let text = turn.text.trim();
        if !text.is_empty() {
            tools.log(short(text));
        }
        if turn.tool_calls.is_empty() {
            break;
        }
        let mut results = Vec::with_capacity(turn.tool_calls.len());
        for call in &turn.tool_calls {
            if cancel.is_cancelled() {
                bail!("cancelado");
            }
            let (content, is_error) = tools.call(&call.name, &call.input).await;
            results.push(ToolResult { id: call.id.clone(), content, is_error });
        }
        if tools.is_finished() {
            break;
        }
        turn = tokio::select! {
            r = session.tool_results(results) => r?,
            _ = cancel.cancelled() => bail!("cancelado"),
        };
    }
    tools.log(format!("tokens: {} entrada / {} saída", tokens.0, tokens.1));
    let summary = tools.summary();
    Ok(if summary.is_empty() { "concluído".into() } else { summary })
}

pub(crate) fn short(text: &str) -> String {
    let line = text.lines().find(|l| !l.trim().is_empty()).unwrap_or(text).trim();
    if line.chars().count() > 220 {
        format!("{}…", line.chars().take(220).collect::<String>())
    } else {
        line.to_string()
    }
}
