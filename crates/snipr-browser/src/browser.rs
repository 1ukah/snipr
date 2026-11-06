//! Lançamento e ciclo de vida do Chrome.
//!
//! Um único processo Chrome, com perfil persistente próprio do Snipr, serve
//! todos os agentes. Cada agente ganha uma janela (= aba ativa, nunca
//! "background", então sem throttling) dentro desse mesmo processo.

use std::{
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::process::{Child, Command};

use crate::{cdp::Cdp, page::Page};

/// Posição usada no modo oculto: a janela existe de verdade (nenhum sinal de
/// headless), só que fora da área visível do monitor.
pub(crate) const HIDDEN_POS: (i64, i64) = (-2400, -2400);
pub(crate) const WINDOW_SIZE: (i64, i64) = (1366, 900);

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BrowserMode {
    /// Chrome com janela real posicionada fora da tela. Mais difícil de detectar.
    Hidden,
    /// `--headless=new`. Um pouco mais leve, um pouco mais detectável.
    Headless,
    /// Janela visível (debug / acompanhar o agente).
    Visible,
}

#[derive(Clone, Debug)]
pub struct LaunchOptions {
    pub chrome_path: Option<PathBuf>,
    pub profile_dir: PathBuf,
    pub mode: BrowserMode,
    /// Bloqueia imagens, fontes e mídia — grande economia de RAM e banda.
    pub block_media: bool,
    /// Agrupa sites em menos processos de renderização.
    pub low_memory: bool,
}

pub struct Browser {
    pub(crate) cdp: Arc<Cdp>,
    child: tokio::sync::Mutex<Option<Child>>,
    pub(crate) opts: LaunchOptions,
    pub(crate) ua_override: Option<UaOverride>,
    pub version: String,
}

#[derive(Clone, Debug)]
pub(crate) struct UaOverride {
    pub user_agent: String,
    pub metadata: Value,
}

/// Procura um Chrome (ou Chromium/Edge) instalado.
pub fn find_chrome() -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    #[cfg(windows)]
    {
        for var in ["LOCALAPPDATA", "PROGRAMFILES", "PROGRAMFILES(X86)"] {
            if let Ok(base) = std::env::var(var) {
                candidates.push(Path::new(&base).join("Google/Chrome/Application/chrome.exe"));
            }
        }
        for var in ["PROGRAMFILES(X86)", "PROGRAMFILES"] {
            if let Ok(base) = std::env::var(var) {
                candidates.push(Path::new(&base).join("Microsoft/Edge/Application/msedge.exe"));
            }
        }
    }
    #[cfg(target_os = "macos")]
    {
        candidates.push("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome".into());
        candidates.push("/Applications/Chromium.app/Contents/MacOS/Chromium".into());
        candidates.push("/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge".into());
    }
    if let Some(p) = candidates.into_iter().find(|p| p.exists()) {
        return Some(p);
    }
    for name in ["google-chrome-stable", "google-chrome", "chromium", "chromium-browser", "microsoft-edge", "chrome"] {
        if let Ok(p) = which::which(name) {
            return Some(p);
        }
    }
    None
}

impl Browser {
    pub async fn launch(opts: LaunchOptions) -> Result<Arc<Self>> {
        let exe = opts
            .chrome_path
            .clone()
            .filter(|p| p.exists())
            .or_else(find_chrome)
            .ok_or_else(|| anyhow!("Chrome não encontrado. Instale o Google Chrome ou informe o caminho nos ajustes."))?;

        std::fs::create_dir_all(&opts.profile_dir).context("criando diretório do perfil")?;
        let port_file = opts.profile_dir.join("DevToolsActivePort");
        let _ = std::fs::remove_file(&port_file);

        let mut disabled_features = vec![
            "Translate",
            "MediaRouter",
            "OptimizationHints",
            "CalculateNativeWinOcclusion",
            "AutofillServerCommunication",
            "InterestFeedContentSuggestions",
        ];
        let mut args: Vec<String> = vec![
            format!("--user-data-dir={}", opts.profile_dir.display()),
            "--remote-debugging-port=0".into(),
            "--no-first-run".into(),
            "--no-default-browser-check".into(),
            "--disable-blink-features=AutomationControlled".into(),
            "--disable-background-timer-throttling".into(),
            "--disable-backgrounding-occluded-windows".into(),
            "--disable-renderer-backgrounding".into(),
            "--disable-session-crashed-bubble".into(),
            "--hide-crash-restore-bubble".into(),
            "--disable-search-engine-choice-screen".into(),
            "--lang=pt-BR".into(),
        ];
        match opts.mode {
            BrowserMode::Hidden => {
                args.push(format!("--window-position={},{}", HIDDEN_POS.0, HIDDEN_POS.1));
                args.push(format!("--window-size={},{}", WINDOW_SIZE.0, WINDOW_SIZE.1));
            }
            BrowserMode::Headless => {
                args.push("--headless=new".into());
                args.push(format!("--window-size={},{}", WINDOW_SIZE.0, WINDOW_SIZE.1));
            }
            BrowserMode::Visible => {
                args.push("--window-size=1280,860".into());
            }
        }
        if opts.low_memory {
            args.push("--renderer-process-limit=4".into());
            // NÃO usar --disable-site-isolation-trials: o sensor do Akamai detecta e
            // bloqueia (testado na Magazine Luiza: 3/3 bloqueios com a flag, 0/3 sem).
            // Processos e serviços de fundo que o Snipr não usa.
            args.push("--disable-component-update".into());
            args.push("--disable-component-extensions-with-background-pages".into());
            args.push("--disable-default-apps".into());
            args.push("--disable-extensions".into());
            args.push("--disable-background-networking".into());
            args.push("--no-pings".into());
            args.push("--process-per-site".into());
            // Composição em software: corta ~25% da RAM e mantém o WebGL na GPU
            // real (o fingerprint não muda, ao contrário de --disable-gpu).
            args.push("--disable-gpu-compositing".into());
            disabled_features.push("SpareRendererForSitePerProcess");
            disabled_features.push("PrefetchPrivacyChanges");
            disabled_features.push("Prerender2");
            disabled_features.push("PreloadMediaEngagementData");
            disabled_features.push("BackForwardCache");
        }
        // Chrome só respeita o último --disable-features, então juntamos tudo.
        args.push(format!("--disable-features={}", disabled_features.join(",")));
        // Flags extras para usuários avançados / experimentos.
        if let Ok(extra) = std::env::var("SNIPR_CHROME_ARGS") {
            args.extend(extra.split_whitespace().map(String::from));
        }
        // Diagnóstico: SNIPR_CHROME_BARE=1 descarta todas as flags opcionais
        // (fica só perfil + porta de depuração + extras de SNIPR_CHROME_ARGS).
        if std::env::var("SNIPR_CHROME_BARE").is_ok_and(|v| v == "1") {
            args.retain(|a| a.starts_with("--user-data-dir=") || a == "--remote-debugging-port=0");
            if let Ok(extra) = std::env::var("SNIPR_CHROME_ARGS") {
                args.extend(extra.split_whitespace().map(String::from));
            }
        }
        args.push("about:blank".into());

        tracing::info!(exe = %exe.display(), mode = ?opts.mode, "lançando navegador");
        let child = Command::new(&exe)
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("falha ao iniciar {}", exe.display()))?;

        let ws_url = wait_devtools_port(&port_file, Duration::from_secs(25)).await?;
        let cdp = Cdp::connect(&ws_url).await?;
        let ver = cdp.call(None, "Browser.getVersion", json!({})).await?;
        let user_agent = ver["userAgent"].as_str().unwrap_or_default().to_string();
        let version = ver["product"].as_str().unwrap_or_default().to_string();

        let ua_override = if user_agent.contains("HeadlessChrome") {
            Some(build_ua_override(&user_agent))
        } else {
            None
        };

        Ok(Arc::new(Self { cdp, child: tokio::sync::Mutex::new(Some(child)), opts, ua_override, version }))
    }

    pub fn mode(&self) -> BrowserMode {
        self.opts.mode
    }

    pub fn is_alive(&self) -> bool {
        !self.cdp.is_closed()
    }

    /// Abre uma nova aba isolada em sua própria janela.
    pub async fn new_page(self: &Arc<Self>) -> Result<Arc<Page>> {
        let mut params = json!({ "url": "about:blank" });
        if self.opts.mode != BrowserMode::Headless {
            params["newWindow"] = json!(true);
            if self.opts.mode == BrowserMode::Hidden {
                params["left"] = json!(HIDDEN_POS.0);
                params["top"] = json!(HIDDEN_POS.1);
                params["width"] = json!(WINDOW_SIZE.0);
                params["height"] = json!(WINDOW_SIZE.1);
            }
        }
        let created = self.cdp.call(None, "Target.createTarget", params).await?;
        let target_id = created["targetId"].as_str().ok_or_else(|| anyhow!("createTarget sem targetId"))?.to_string();
        let attached = self
            .cdp
            .call(None, "Target.attachToTarget", json!({ "targetId": target_id, "flatten": true }))
            .await?;
        let session = attached["sessionId"].as_str().ok_or_else(|| anyhow!("attach sem sessionId"))?.to_string();
        let page = Arc::new(Page::new(self.clone(), session, target_id));
        page.init().await?;
        Ok(page)
    }

    pub async fn close(&self) {
        let _ = self
            .cdp
            .call_timeout(None, "Browser.close", json!({}), Duration::from_secs(5))
            .await;
        if let Some(mut child) = self.child.lock().await.take() {
            let _ = tokio::time::timeout(Duration::from_secs(3), child.wait()).await;
            let _ = child.kill().await;
        }
    }
}

async fn wait_devtools_port(port_file: &Path, max: Duration) -> Result<String> {
    let start = Instant::now();
    loop {
        if let Ok(s) = std::fs::read_to_string(port_file) {
            let mut lines = s.lines();
            if let (Some(port), Some(path)) = (lines.next(), lines.next()) {
                if !port.trim().is_empty() && path.starts_with('/') {
                    return Ok(format!("ws://127.0.0.1:{}{}", port.trim(), path.trim()));
                }
            }
        }
        if start.elapsed() > max {
            bail!(
                "o Chrome não abriu a porta de depuração. Se já houver um Snipr aberto, feche-o; \
                 o perfil do Snipr não pode ser usado por dois processos ao mesmo tempo."
            );
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// No headless o user-agent e os Client Hints carregam "HeadlessChrome".
/// Reescrevemos ambos de forma coerente com o Chrome normal da mesma versão.
fn build_ua_override(ua: &str) -> UaOverride {
    let user_agent = ua.replace("HeadlessChrome", "Chrome");
    let full = user_agent
        .split("Chrome/")
        .nth(1)
        .and_then(|s| s.split_whitespace().next())
        .unwrap_or("131.0.0.0")
        .to_string();
    let major = full.split('.').next().unwrap_or("131").to_string();
    let (platform, platform_version) = if cfg!(windows) {
        ("Windows", "10.0.0")
    } else if cfg!(target_os = "macos") {
        ("macOS", "14.0.0")
    } else {
        ("Linux", "6.5.0")
    };
    let arch = if cfg!(target_arch = "aarch64") { "arm" } else { "x86" };
    let metadata = json!({
        "brands": [
            { "brand": "Google Chrome", "version": major },
            { "brand": "Chromium", "version": major },
            { "brand": "Not_A Brand", "version": "24" }
        ],
        "fullVersionList": [
            { "brand": "Google Chrome", "version": full },
            { "brand": "Chromium", "version": full },
            { "brand": "Not_A Brand", "version": "24.0.0.0" }
        ],
        "platform": platform,
        "platformVersion": platform_version,
        "architecture": arch,
        "model": "",
        "mobile": false,
        "bitness": "64",
        "wow64": false
    });
    UaOverride { user_agent, metadata }
}
