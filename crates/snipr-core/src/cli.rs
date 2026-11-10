use std::{path::PathBuf, process::Stdio, sync::Arc, time::Duration};

use anyhow::{anyhow, bail, Context, Result};
use serde::Serialize;
use serde_json::{json, Value};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::Command,
};
use tokio_util::sync::CancellationToken;
use ts_rs::TS;

use crate::{agent::short, config::Provider, tools::ToolBox, types::Usage};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CliKind {
    Claude,
    Codex,
    Antigravity,
}

impl CliKind {
    pub fn for_provider(p: Provider) -> Option<Self> {
        match p {
            Provider::Anthropic => Some(Self::Claude),
            Provider::Openai => Some(Self::Codex),
            Provider::Google => Some(Self::Antigravity),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Claude => "Claude Code",
            Self::Codex => "Codex",
            Self::Antigravity => "Antigravity CLI",
        }
    }

    fn binary_name(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Antigravity => "agy",
        }
    }

    fn home_fallbacks(self) -> &'static [&'static str] {
        match self {
            Self::Claude => &[".local/bin/claude.exe", ".local/bin/claude", ".claude/local/claude"],
            Self::Codex => &[".local/bin/codex.exe", ".local/bin/codex"],
            Self::Antigravity => &[".local/bin/agy.exe", ".local/bin/agy"],
        }
    }

    fn login_args(self) -> &'static [&'static str] {
        match self {
            Self::Claude => &["auth", "login"],
            Self::Codex => &["login"],
            Self::Antigravity => &[],
        }
    }
}

pub fn find(kind: CliKind, custom: &str) -> Option<PathBuf> {
    if !custom.trim().is_empty() {
        let p = PathBuf::from(custom.trim());
        return p.exists().then_some(p);
    }
    if let Ok(p) = which::which(kind.binary_name()) {
        return Some(p);
    }
    let home = dirs::home_dir()?;
    kind.home_fallbacks().iter().map(|p| home.join(p)).find(|p| p.exists())
}

#[derive(Clone, Debug, Serialize, TS)]
#[ts(export)]
pub struct CliStatus {
    pub provider: Provider,
    pub tool: String,
    pub installed: bool,
    pub logged_in: Option<bool>,
}

pub async fn status(kind: CliKind, provider: Provider, custom: &str) -> CliStatus {
    let base = CliStatus { provider, tool: kind.label().into(), installed: false, logged_in: None };
    let Some(bin) = find(kind, custom) else { return base };
    let logged_in = match kind {
        CliKind::Claude => quick(&bin, &["auth", "status"]).await.and_then(|(_, out)| {
            serde_json::from_str::<Value>(&out).ok().and_then(|v| v["loggedIn"].as_bool())
        }),
        CliKind::Codex => quick(&bin, &["login", "status"]).await.map(|(ok, out)| ok && out.to_lowercase().contains("logged in")),
        CliKind::Antigravity => None,
    };
    CliStatus { installed: true, logged_in, ..base }
}

/// Modelos que o CLI aceita na conta logada.
pub async fn models(kind: CliKind, custom: &str) -> Result<Vec<String>> {
    // O Claude Code aceita o apelido da família e resolve para a versão atual.
    if kind == CliKind::Claude {
        return Ok(["opus", "sonnet", "haiku"].map(String::from).to_vec());
    }
    let bin = find(kind, custom).ok_or_else(|| anyhow!("{} não está instalado.", kind.label()))?;
    let args: &[&str] = if kind == CliKind::Codex { &["debug", "models"] } else { &["models"] };
    let (_, out) = quick(&bin, args).await.ok_or_else(|| anyhow!("O {} não listou os modelos.", kind.label()))?;
    let list: Vec<String> = match kind {
        CliKind::Codex => {
            let start = out.find('{').unwrap_or(0);
            let v = serde_json::Deserializer::from_str(&out[start..]).into_iter::<Value>().next().and_then(|v| v.ok()).unwrap_or_default();
            v["models"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|m| m["visibility"].as_str().is_none_or(|s| s == "list"))
                .filter_map(|m| m["slug"].as_str().map(String::from))
                .collect()
        }
        _ => out
            .lines()
            .filter_map(|l| l.split_once('\t'))
            .map(|(id, _)| id.trim().to_string())
            .filter(|id| !id.is_empty() && !id.contains(' '))
            .collect(),
    };
    if list.is_empty() {
        bail!("O {} não listou os modelos.", kind.label());
    }
    Ok(list)
}

async fn quick(bin: &PathBuf, args: &[&str]) -> Option<(bool, String)> {
    let mut cmd = Command::new(bin);
    cmd.args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    no_window(&mut cmd);
    let out = tokio::time::timeout(Duration::from_secs(15), cmd.output()).await.ok()?.ok()?;
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    Some((out.status.success(), text))
}

pub fn open_login(kind: CliKind, custom: &str) -> Result<()> {
    let bin = find(kind, custom).ok_or_else(|| anyhow!("{} não está instalado.", kind.label()))?;
    let line = std::iter::once(format!("\"{}\"", bin.display()))
        .chain(kind.login_args().iter().map(|a| a.to_string()))
        .collect::<Vec<_>>()
        .join(" ");
    open_terminal(&line)
}

#[cfg(target_os = "macos")]
fn open_terminal(line: &str) -> Result<()> {
    let script = line.replace('\\', "\\\\").replace('"', "\\\"");
    std::process::Command::new("osascript")
        .args(["-e", &format!("tell application \"Terminal\" to do script \"{script}\""), "-e", "tell application \"Terminal\" to activate"])
        .spawn()
        .context("não foi possível abrir o Terminal")?;
    Ok(())
}

#[cfg(windows)]
fn open_terminal(line: &str) -> Result<()> {
    std::process::Command::new("cmd").args(["/C", "start", "", "cmd", "/K", line]).spawn().context("não foi possível abrir o terminal")?;
    Ok(())
}

#[cfg(all(unix, not(target_os = "macos")))]
fn open_terminal(line: &str) -> Result<()> {
    let candidates: [(&str, &[&str]); 5] = [
        ("x-terminal-emulator", &["-e", "sh", "-c"]),
        ("gnome-terminal", &["--", "sh", "-c"]),
        ("konsole", &["-e", "sh", "-c"]),
        ("xfce4-terminal", &["-x", "sh", "-c"]),
        ("xterm", &["-e", "sh", "-c"]),
    ];
    let keep = format!("{line}; exec $SHELL");
    for (term, args) in candidates {
        if which::which(term).is_ok() {
            std::process::Command::new(term).args(args).arg(&keep).spawn().context("não foi possível abrir o terminal")?;
            return Ok(());
        }
    }
    bail!("Nenhum terminal encontrado. Execute no terminal: {line}")
}

fn no_window(_cmd: &mut Command) {
    #[cfg(windows)]
    _cmd.creation_flags(0x0800_0000);
}

pub struct CliRun {
    pub kind: CliKind,
    pub binary: PathBuf,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub oauth_token: Option<String>,
    pub dir: PathBuf,
}

pub struct Mcp {
    pub url: String,
    pub token: String,
}

enum Input {
    Stdin(String),
    Arg(String),
}

fn build(run: &CliRun, system: &str, prompt: String, mcp: Option<&Mcp>, last_message: &PathBuf) -> Result<(Command, Input)> {
    std::fs::create_dir_all(&run.dir)?;
    let mut cmd = Command::new(&run.binary);
    let model = run.model.as_deref().filter(|m| !m.is_empty());
    let effort = run.effort.as_deref().filter(|e| !e.is_empty());
    let input = match run.kind {
        CliKind::Claude => {
            let sys_path = run.dir.join("system.md");
            std::fs::write(&sys_path, system)?;
            cmd.arg("-p").arg("--system-prompt-file").arg(&sys_path).args([
                "--no-session-persistence",
                "--disable-slash-commands",
                "--permission-mode",
                "dontAsk",
                "--tools",
                "",
            ]);
            if let Some(m) = mcp {
                let mcp_path = run.dir.join("mcp.json");
                let cfg = json!({ "mcpServers": { "snipr": {
                    "type": "http",
                    "url": m.url,
                    "headers": { "Authorization": format!("Bearer {}", m.token) }
                }}});
                std::fs::write(&mcp_path, serde_json::to_vec_pretty(&cfg)?)?;
                cmd.args(["--output-format", "stream-json", "--verbose"])
                    .arg("--mcp-config")
                    .arg(&mcp_path)
                    .args(["--strict-mcp-config", "--allowedTools", "mcp__snipr"]);
            } else {
                cmd.args(["--output-format", "stream-json", "--verbose"]);
            }
            if let Some(m) = model {
                cmd.args(["--model", m]);
            }
            if let Some(e) = effort {
                cmd.args(["--effort", e]);
            }
            if let Some(t) = &run.oauth_token {
                cmd.env("CLAUDE_CODE_OAUTH_TOKEN", t).env_remove("ANTHROPIC_API_KEY");
            }
            Input::Stdin(prompt)
        }
        CliKind::Codex => {
            cmd.arg("exec")
                .args(["--skip-git-repo-check", "--ephemeral", "--ignore-user-config", "--ignore-rules", "--color", "never"])
                .args(["-s", "read-only", "-c", "approval_policy=\"never\"", "-c", "web_search=\"disabled\""])
                .arg("-C")
                .arg(&run.dir)
                .arg("-o")
                .arg(last_message);
            for f in ["shell_tool", "unified_exec", "view_image", "browser_use", "browser_use_external", "computer_use", "in_app_browser", "apps", "plugins"] {
                cmd.args(["--disable", f]);
            }
            cmd.arg("--json");
            if let Some(m) = mcp {
                cmd.args(["-c", &format!("mcp_servers.snipr.url={}", toml_str(&m.url))])
                    .args(["-c", "mcp_servers.snipr.bearer_token_env_var=\"SNIPR_MCP_TOKEN\""])
                    .args(["-c", "mcp_servers.snipr.required=true"])
                    .args(["-c", "mcp_servers.snipr.default_tools_approval_mode=\"approve\""])
                    .args(["-c", "mcp_servers.snipr.tool_timeout_sec=900"])
                    .env("SNIPR_MCP_TOKEN", &m.token);
            }
            if let Some(m) = model {
                cmd.args(["-m", m]);
            }
            if let Some(e) = effort {
                cmd.args(["-c", &format!("model_reasoning_effort={}", toml_str(e))]);
            }
            cmd.arg("-");
            Input::Stdin(format!("{system}\n\n---\n\n{prompt}"))
        }
        CliKind::Antigravity => {
            if let Some(m) = mcp {
                let cfg_dir = run.dir.join(".agents");
                std::fs::create_dir_all(&cfg_dir)?;
                let cfg = json!({ "mcpServers": { "snipr": {
                    "serverUrl": m.url,
                    "headers": { "Authorization": format!("Bearer {}", m.token) },
                    "timeoutSeconds": 900
                }}});
                std::fs::write(cfg_dir.join("mcp_config.json"), serde_json::to_vec_pretty(&cfg)?)?;
                allow_antigravity_mcp()?;
            }
            cmd.args(["--print-timeout", "60m"]);
            if let Some(m) = model {
                cmd.args(["--model", m]);
            }
            if let Some(e) = effort {
                cmd.args(["--effort", e]);
            }
            Input::Arg(format!("{system}\n\n---\n\n{prompt}"))
        }
    };
    cmd.current_dir(&run.dir).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    no_window(&mut cmd);
    Ok((cmd, input))
}

fn toml_str(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

fn allow_antigravity_mcp() -> Result<()> {
    let Some(home) = dirs::home_dir() else { return Ok(()) };
    let path = home.join(".gemini").join("antigravity-cli").join("settings.json");
    let mut v: Value = std::fs::read_to_string(&path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_else(|| json!({}));
    if !v.is_object() {
        return Ok(());
    }
    let rule = "mcp(snipr/*)";
    let allow = v.pointer("/permissions/allow").and_then(Value::as_array).cloned().unwrap_or_default();
    if allow.iter().any(|r| r == rule) {
        return Ok(());
    }
    if !v["permissions"].is_object() {
        v["permissions"] = json!({});
    }
    let mut allow = allow;
    allow.push(json!(rule));
    v["permissions"]["allow"] = Value::Array(allow);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&path, serde_json::to_vec_pretty(&v)?)?;
    Ok(())
}

struct Output {
    ok: bool,
    status: String,
    last_text: String,
    stderr: String,
}

async fn exec(
    kind: CliKind,
    mut cmd: Command,
    input: Input,
    cancel: &CancellationToken,
    parse_json: bool,
    mut on_line: impl FnMut(&Value),
) -> Result<Output> {
    if let Input::Arg(p) = &input {
        cmd.arg("-p").arg(p);
    }
    let mut child = cmd.spawn().with_context(|| format!("falha ao iniciar o {}", kind.label()))?;
    {
        let mut stdin = child.stdin.take().ok_or_else(|| anyhow!("sem stdin"))?;
        if let Input::Stdin(p) = &input {
            stdin.write_all(p.as_bytes()).await?;
        }
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
    let mut text = String::new();
    loop {
        let line = tokio::select! {
            l = lines.next_line() => l?,
            _ = cancel.cancelled() => {
                let _ = child.kill().await;
                bail!("cancelado");
            }
        };
        let Some(line) = line else { break };
        match parse_json.then(|| serde_json::from_str::<Value>(&line).ok()).flatten() {
            Some(v) if v.is_object() => on_line(&v),
            _ => {
                text.push_str(&line);
                text.push('\n');
            }
        }
    }
    let status = child.wait().await?;
    let stderr = err_task.await.unwrap_or_default();
    Ok(Output { ok: status.success(), status: status.to_string(), last_text: text.trim().to_string(), stderr })
}

fn usage_of(kind: CliKind, v: &Value, model: &Option<String>) -> Option<Usage> {
    let u = &v["usage"];
    let n = |k: &str| u[k].as_u64().unwrap_or(0);
    match (kind, v["type"].as_str()) {
        (CliKind::Claude, Some("result")) => Some(Usage {
            input_tokens: n("input_tokens") + n("cache_read_input_tokens") + n("cache_creation_input_tokens"),
            output_tokens: n("output_tokens"),
            cost_usd: v["total_cost_usd"].as_f64(),
            model: model.clone(),
        }),
        (CliKind::Codex, Some("turn.completed")) => {
            Some(Usage { input_tokens: n("input_tokens"), output_tokens: n("output_tokens"), cost_usd: None, model: model.clone() })
        }
        _ => None,
    }
}

fn init_model(kind: CliKind, v: &Value) -> Option<String> {
    (kind == CliKind::Claude && v["type"] == "system" && v["subtype"] == "init").then(|| v["model"].as_str().map(String::from)).flatten()
}

fn failure(kind: CliKind, out: &Output) -> anyhow::Error {
    let all = format!("{}\n{}", out.stderr, out.last_text);
    let low = all.to_lowercase();
    if low.contains("authentication required") || low.contains("not logged in") || low.contains("please log in") || low.contains("login required") {
        return anyhow!("Faça o login no {} em Ajustes > IA.", kind.label());
    }
    let tail: String = all.lines().filter(|l| !l.trim().is_empty()).rev().take(4).collect::<Vec<_>>().join(" | ");
    anyhow!("{} saiu com {}: {}", kind.label(), out.status, tail)
}

pub async fn run_agent(run: CliRun, system: &str, task: String, tools: Arc<ToolBox>, mcp: Mcp, cancel: CancellationToken) -> Result<String> {
    let last_path = run.dir.join("last-message.txt");
    let _ = std::fs::remove_file(&last_path);
    let (cmd, input) = build(&run, system, task, Some(&mcp), &last_path)?;
    let kind = run.kind;
    let mut result: Option<(bool, String)> = None;
    let mut last_message = String::new();
    let mut model = run.model.clone();
    let out = exec(kind, cmd, input, &cancel, true, |v| {
        if let Some(m) = init_model(kind, v) {
            model = Some(m);
        }
        if let Some(u) = usage_of(kind, v, &model) {
            tools.usage(u);
        }
        match kind {
            CliKind::Claude => match v["type"].as_str() {
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
                    if let Some(cost) = v["total_cost_usd"].as_f64() {
                        tools.log(format!("custo estimado da sessão: US$ {cost:.3}"));
                    }
                    result = Some((is_error, v["result"].as_str().unwrap_or_default().to_string()));
                }
                Some("system") if v["subtype"] == "init" => {
                    let ok = v["mcp_servers"].as_array().into_iter().flatten().any(|s| s["name"] == "snipr" && s["status"] == "connected");
                    if !ok {
                        tools.log("aviso: o Claude Code não confirmou a conexão MCP com o Snipr");
                    }
                }
                _ => {}
            },
            CliKind::Codex => match v["type"].as_str() {
                Some("item.completed") if v["item"]["type"] == "agent_message" => {
                    let t = v["item"]["text"].as_str().unwrap_or_default().trim().to_string();
                    if !t.is_empty() {
                        tools.log(short(&t));
                        last_message = t;
                    }
                }
                Some("turn.failed") => result = Some((true, v["error"]["message"].as_str().unwrap_or_default().to_string())),
                Some("error") => result = Some((true, v["message"].as_str().unwrap_or_default().to_string())),
                _ => {}
            },
            CliKind::Antigravity => {}
        }
    })
    .await?;
    let summary = tools.summary();
    match result {
        Some((true, text)) => bail!("{} terminou com erro: {}", kind.label(), short(&text)),
        _ if !out.ok => Err(failure(kind, &out)),
        Some((false, text)) if summary.is_empty() => Ok(short(&text)),
        _ if !summary.is_empty() => Ok(summary),
        _ => {
            let fallback = std::fs::read_to_string(&last_path).unwrap_or(last_message);
            let text = if fallback.trim().is_empty() { out.last_text } else { fallback };
            Ok(short(&text))
        }
    }
}

pub async fn run_text(run: CliRun, system: &str, prompt: String, cancel: CancellationToken) -> Result<(String, Usage)> {
    let last_path = run.dir.join("last-message.txt");
    let _ = std::fs::remove_file(&last_path);
    let (cmd, input) = build(&run, system, prompt, None, &last_path)?;
    let kind = run.kind;
    let mut model = run.model.clone();
    let mut usage = Usage::default();
    let mut result = String::new();
    let out = exec(kind, cmd, input, &cancel, kind != CliKind::Antigravity, |v| {
        if let Some(m) = init_model(kind, v) {
            model = Some(m);
        }
        if let Some(u) = usage_of(kind, v, &model) {
            usage.input_tokens += u.input_tokens;
            usage.output_tokens += u.output_tokens;
            usage.cost_usd = u.cost_usd.map(|c| c + usage.cost_usd.unwrap_or(0.0)).or(usage.cost_usd);
            usage.model = u.model;
        }
        if kind == CliKind::Claude && v["type"] == "result" {
            result = v["result"].as_str().unwrap_or_default().to_string();
        }
    })
    .await?;
    if !out.ok {
        return Err(failure(kind, &out));
    }
    let from_file = std::fs::read_to_string(&last_path).unwrap_or_default();
    let text = [result, from_file].into_iter().find(|t| !t.trim().is_empty()).unwrap_or(out.last_text);
    Ok((text, usage))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        mcp::McpServer,
        sites::default_sites,
        tools::{HumanGates, Sink, ToolBoxInit},
        types::ConditionFilter,
    };
    use snipr_browser::{Browser, BrowserMode, LaunchOptions};

    #[tokio::test]
    #[ignore]
    async fn cli_conecta_no_mcp() {
        let browser = Browser::launch(LaunchOptions {
            chrome_path: None,
            profile_dir: std::env::temp_dir().join("snipr-cli-test-profile"),
            mode: BrowserMode::Hidden,
            block_media: true,
            low_memory: true,
        })
        .await
        .unwrap();
        let server = McpServer::start().await.unwrap();
        let only = std::env::var("SNIPR_TEST_CLI").unwrap_or_default();
        for kind in [CliKind::Claude, CliKind::Codex, CliKind::Antigravity] {
            if !only.is_empty() && !kind.label().to_lowercase().contains(&only) {
                continue;
            }
            let Some(binary) = find(kind, "") else {
                println!("{}: não instalado", kind.label());
                continue;
            };
            let page = browser.new_page().await.unwrap();
            let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
            let agent = format!("teste-{}", kind.binary_name());
            let sites = default_sites();
            let sink = Arc::new(Sink { job: "teste".into(), events: tx, listings: Default::default(), filter: ConditionFilter::Any, max_price: None });
            let tools = Arc::new(ToolBox::new(ToolBoxInit {
                agent: agent.clone(),
                page: page.clone(),
                sites: sites.clone(),
                all_sites: sites,
                sessions: Default::default(),
                on_session: Arc::new(|_, _| {}),
                gates: Arc::new(HumanGates::default()),
                sink,
            }));
            server.register(&agent, tools.clone());
            let run = CliRun {
                kind,
                binary,
                model: None,
                effort: None,
                oauth_token: None,
                dir: std::env::temp_dir().join("snipr-cli-test").join(&agent),
            };
            let mcp = Mcp { url: server.url_for(&agent), token: server.token().to_string() };
            let started = std::time::Instant::now();
            let r = run_agent(
                run,
                "Você testa uma conexão de ferramentas.",
                "Chame a ferramenta finish com summary igual a 'conexao-ok'. Não use outras ferramentas.".into(),
                tools.clone(),
                mcp,
                CancellationToken::new(),
            )
            .await;
            println!("{}: {:?} em {:.1}s", kind.label(), r, started.elapsed().as_secs_f64());
            assert_eq!(tools.summary(), "conexao-ok", "{} não chamou a ferramenta finish pelo MCP", kind.label());
            server.unregister(&agent);
            page.close().await;
        }
        browser.close().await;
    }
}
