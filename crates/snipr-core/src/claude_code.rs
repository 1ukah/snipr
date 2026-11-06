//! Provedor "Claude Code": roda o CLI oficial (`claude -p`) como agente.
//!
//! O Claude Code autentica com a assinatura do usuário (login já feito no
//! CLI, ou o token de `claude setup-token` guardado no cofre do Snipr e
//! repassado via `CLAUDE_CODE_OAUTH_TOKEN`). As ferramentas do navegador
//! chegam até ele pelo servidor MCP local do Snipr. Ferramentas embutidas
//! (Bash, arquivos, web) ficam desligadas: ele só controla o navegador.

use std::{path::PathBuf, process::Stdio, sync::Arc};

use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::Command,
};
use tokio_util::sync::CancellationToken;

use crate::{agent::short, tools::ToolBox};

pub struct ClaudeCodeRun {
    pub binary: PathBuf,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub oauth_token: Option<String>,
    pub work_dir: PathBuf,
    pub mcp_url: String,
    pub mcp_token: String,
}

pub fn find_claude(custom: &str) -> Option<PathBuf> {
    if !custom.trim().is_empty() {
        let p = PathBuf::from(custom.trim());
        return p.exists().then_some(p);
    }
    if let Ok(p) = which::which("claude") {
        return Some(p);
    }
    let home = dirs::home_dir()?;
    [".local/bin/claude.exe", ".local/bin/claude", ".claude/local/claude"]
        .iter()
        .map(|p| home.join(p))
        .find(|p| p.exists())
}

pub async fn run(cfg: ClaudeCodeRun, system: &str, task: String, tools: Arc<ToolBox>, cancel: CancellationToken) -> Result<String> {
    let dir = cfg.work_dir.join(&tools.agent);
    std::fs::create_dir_all(&dir)?;
    let mcp_path = dir.join("mcp.json");
    let sys_path = dir.join("system.md");
    let mcp = json!({ "mcpServers": { "snipr": {
        "type": "http",
        "url": cfg.mcp_url,
        "headers": { "Authorization": format!("Bearer {}", cfg.mcp_token) }
    }}});
    std::fs::write(&mcp_path, serde_json::to_vec_pretty(&mcp)?)?;
    std::fs::write(&sys_path, system)?;

    let mut cmd = Command::new(&cfg.binary);
    cmd.arg("-p")
        .args(["--output-format", "stream-json", "--verbose"])
        .arg("--mcp-config")
        .arg(&mcp_path)
        .arg("--strict-mcp-config")
        .args(["--tools", ""])
        .args(["--allowedTools", "mcp__snipr"])
        .args(["--permission-mode", "dontAsk"])
        .arg("--system-prompt-file")
        .arg(&sys_path)
        .arg("--no-session-persistence")
        .arg("--disable-slash-commands");
    if let Some(m) = cfg.model.as_deref().filter(|m| !m.is_empty()) {
        cmd.args(["--model", m]);
    }
    if let Some(e) = cfg.effort.as_deref().filter(|e| !e.is_empty()) {
        cmd.args(["--effort", e]);
    }
    if let Some(t) = &cfg.oauth_token {
        // Garante que a assinatura (OAuth) seja usada, não uma API key do ambiente.
        cmd.env("CLAUDE_CODE_OAUTH_TOKEN", t).env_remove("ANTHROPIC_API_KEY");
    }
    cmd.current_dir(&dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW: sem janela de console piscando

    let mut child = cmd.spawn().with_context(|| format!("falha ao iniciar {}", cfg.binary.display()))?;
    {
        let mut stdin = child.stdin.take().ok_or_else(|| anyhow!("sem stdin"))?;
        stdin.write_all(task.as_bytes()).await?;
        stdin.shutdown().await?;
    }
    let stdout = child.stdout.take().ok_or_else(|| anyhow!("sem stdout"))?;
    let mut stderr = child.stderr.take().ok_or_else(|| anyhow!("sem stderr"))?;
    let err_task = tokio::spawn(async move {
        let mut s = String::new();
        let _ = stderr.read_to_string(&mut s).await;
        s
    });

    let mut lines = BufReader::new(stdout).lines();
    let mut result: Option<(bool, String)> = None;
    loop {
        let line = tokio::select! {
            l = lines.next_line() => l?,
            _ = cancel.cancelled() => {
                let _ = child.kill().await;
                bail!("cancelado");
            }
        };
        let Some(line) = line else { break };
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        match v["type"].as_str() {
            Some("assistant") => {
                for block in v["message"]["content"].as_array().into_iter().flatten() {
                    if block["type"] == "text" {
                        let t = block["text"].as_str().unwrap_or_default().trim();
                        if !t.is_empty() {
                            tools.log(short(t));
                        }
                    }
                }
            }
            Some("result") => {
                let is_error = v["is_error"].as_bool().unwrap_or(false);
                let text = v["result"].as_str().unwrap_or_default().to_string();
                if let Some(cost) = v["total_cost_usd"].as_f64() {
                    tools.log(format!("custo estimado da sessão: US$ {cost:.3}"));
                }
                result = Some((is_error, text));
            }
            Some("system") if v["subtype"] == "init" => {
                let ok = v["mcp_servers"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .any(|s| s["name"] == "snipr" && s["status"] == "connected");
                if !ok {
                    tools.log("aviso: o Claude Code não confirmou a conexão MCP com o Snipr");
                }
            }
            _ => {}
        }
    }
    let status = child.wait().await?;
    let stderr = err_task.await.unwrap_or_default();
    match result {
        Some((false, text)) => {
            let s = tools.summary();
            Ok(if s.is_empty() { short(&text) } else { s })
        }
        Some((true, text)) => bail!("Claude Code terminou com erro: {}", short(&text)),
        None if !status.success() => {
            let tail: String = stderr.lines().rev().take(4).collect::<Vec<_>>().join(" | ");
            bail!("Claude Code saiu com {status}: {tail}")
        }
        None => Ok(tools.summary()),
    }
}
