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
/// Máximo de abas estacionadas para reuso (igual ao máximo de abas simultâneas).
const POOL_MAX: usize = 8;

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
    pid: Option<u32>,
    revealed: std::sync::atomic::AtomicBool,
    startup: parking_lot::Mutex<Vec<String>>,
    shown: parking_lot::Mutex<Option<String>>,
    /// Abas estacionadas em about:blank, prontas para reuso.
    pool: parking_lot::Mutex<Vec<Arc<Page>>>,
    pub(crate) opts: LaunchOptions,
    pub(crate) ua_override: Option<UaOverride>,
    pub version: String,
}

#[derive(Clone, Debug)]
pub(crate) struct UaOverride {
    pub user_agent: String,
    pub metadata: Value,
}

/// Navegador baseado no Chromium encontrado no sistema.
#[derive(Clone, Debug, Serialize)]
pub struct BrowserInfo {
    pub name: String,
    pub path: PathBuf,
    pub version: Option<String>,
}

/// Navegadores compatíveis (Chromium + CDP), em ordem de preferência. O Chrome
/// vem primeiro: é o mais comum e o que as lojas menos estranham. O Brave fica
/// por último porque randomiza o fingerprint (canvas, WebGL), o que alguns
/// sistemas anti-bot tratam como suspeito.
pub fn detect_browsers() -> Vec<BrowserInfo> {
    let mut found: Vec<(String, PathBuf)> = Vec::new();
    let mut push = |name: &str, p: PathBuf| {
        if p.is_file() && !found.iter().any(|(_, x)| x == &p) {
            found.push((name.to_string(), p));
        }
    };
    #[cfg(windows)]
    {
        let bases: Vec<PathBuf> = ["LOCALAPPDATA", "PROGRAMFILES", "PROGRAMFILES(X86)"]
            .iter()
            .filter_map(|v| std::env::var(v).ok().map(PathBuf::from))
            .collect();
        for (name, rel) in [
            ("Google Chrome", "Google/Chrome/Application/chrome.exe"),
            ("Chromium", "Chromium/Application/chrome.exe"),
            ("Microsoft Edge", "Microsoft/Edge/Application/msedge.exe"),
            ("Brave", "BraveSoftware/Brave-Browser/Application/brave.exe"),
        ] {
            for b in &bases {
                push(name, b.join(rel));
            }
        }
    }
    #[cfg(target_os = "macos")]
    {
        let home = std::env::var("HOME").map(PathBuf::from).unwrap_or_default();
        for (name, rel) in [
            ("Google Chrome", "Google Chrome.app/Contents/MacOS/Google Chrome"),
            ("Chromium", "Chromium.app/Contents/MacOS/Chromium"),
            ("Microsoft Edge", "Microsoft Edge.app/Contents/MacOS/Microsoft Edge"),
            ("Brave", "Brave Browser.app/Contents/MacOS/Brave Browser"),
        ] {
            push(name, Path::new("/Applications").join(rel));
            push(name, home.join("Applications").join(rel));
        }
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        for (name, bins) in [
            ("Google Chrome", &["google-chrome-stable", "google-chrome"][..]),
            ("Chromium", &["chromium", "chromium-browser"][..]),
            ("Microsoft Edge", &["microsoft-edge-stable", "microsoft-edge"][..]),
            ("Brave", &["brave-browser", "brave"][..]),
        ] {
            for b in bins {
                if let Ok(p) = which::which(b) {
                    push(name, p);
                }
            }
        }
    }
    found.into_iter().map(|(name, path)| BrowserInfo { version: browser_version(&path), name, path }).collect()
}

/// Versão sem abrir janela: no Windows o `--version` abre o navegador, então
/// lemos a pasta de versão ao lado do executável.
fn browser_version(exe: &Path) -> Option<String> {
    let is_version = |s: &str| s.split('.').count() == 4 && s.split('.').all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()));
    if cfg!(windows) {
        let dir = exe.parent()?;
        let mut v: Vec<String> = std::fs::read_dir(dir)
            .ok()?
            .flatten()
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|n| is_version(n))
            .collect();
        v.sort_by_key(|s| s.split('.').map(|p| p.parse::<u32>().unwrap_or(0)).collect::<Vec<_>>());
        return v.pop();
    }
    let out = std::process::Command::new(exe).arg("--version").stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
    String::from_utf8_lossy(&out.stdout).split_whitespace().find(|w| is_version(w)).map(String::from)
}

/// Primeiro navegador compatível encontrado.
pub fn find_chrome() -> Option<PathBuf> {
    detect_browsers().into_iter().next().map(|b| b.path)
}

/// Nome da marca nos Client Hints, coerente com o executável.
fn brand_for(exe: &Path) -> &'static str {
    let name = exe.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_lowercase();
    if name.contains("edge") {
        "Microsoft Edge"
    } else if name.contains("brave") {
        "Brave"
    } else if name.contains("chromium") {
        "Chromium"
    } else {
        "Google Chrome"
    }
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
                if !cfg!(target_os = "macos") {
                    args.push(format!("--window-position={},{}", HIDDEN_POS.0, HIDDEN_POS.1));
                }
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
            // Sem --disable-gpu-compositing: ele economiza RAM, mas passa a
            // composição para a CPU. O Snipr prefere gastar RAM a gastar CPU.
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

        tracing::info!(exe = %exe.display(), mode = ?opts.mode, "lançando navegador");
        let hides_app = cfg!(target_os = "macos") && opts.mode == BrowserMode::Hidden;
        if let Some(stale) = find_pid(&opts.profile_dir) {
            tracing::warn!(pid = stale, "encerrando um Chrome antigo do Snipr");
            kill_pid(stale);
            let start = Instant::now();
            while pid_alive(stale) && start.elapsed() < Duration::from_secs(5) {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
        let (child, mut pid) = spawn(&exe, &args, hides_app)?;

        let ws_url = match wait_devtools_port(&port_file, Duration::from_secs(25)).await {
            Ok(u) => u,
            Err(e) => {
                if let Some(p) = pid.or_else(|| find_pid(&opts.profile_dir)) {
                    kill_pid(p);
                }
                return Err(e);
            }
        };
        if pid.is_none() {
            pid = find_pid(&opts.profile_dir);
        }
        let cdp = Cdp::connect(&ws_url).await?;
        let startup = page_targets(&cdp).await;
        let ver = cdp.call(None, "Browser.getVersion", json!({})).await?;
        let user_agent = ver["userAgent"].as_str().unwrap_or_default().to_string();
        let version = ver["product"].as_str().unwrap_or_default().to_string();

        let ua_override = if user_agent.contains("HeadlessChrome") {
            Some(build_ua_override(&user_agent, brand_for(&exe)))
        } else {
            None
        };

        let browser = Arc::new(Self {
            cdp,
            child: tokio::sync::Mutex::new(child),
            pid,
            revealed: std::sync::atomic::AtomicBool::new(false),
            startup: parking_lot::Mutex::new(startup),
            shown: parking_lot::Mutex::new(None),
            pool: parking_lot::Mutex::new(Vec::new()),
            opts,
            ua_override,
            version,
        });
        browser.keep_hidden();
        if browser.hides_app() {
            let weak = Arc::downgrade(&browser);
            tokio::spawn(async move {
                loop {
                    // Novas janelas já chamam keep_hidden; este laço só cobre o
                    // caso raro de o macOS reexibir o app sozinho.
                    tokio::time::sleep(Duration::from_millis(1000)).await;
                    let Some(b) = weak.upgrade() else { break };
                    if b.cdp.is_closed() {
                        break;
                    }
                    b.keep_hidden();
                }
            });
        }
        Ok(browser)
    }

    pub fn app_hidden(&self) -> Option<bool> {
        #[cfg(target_os = "macos")]
        if let Some(pid) = self.pid {
            return crate::macos::is_hidden(pid);
        }
        None
    }

    fn hides_app(&self) -> bool {
        cfg!(target_os = "macos") && self.opts.mode == BrowserMode::Hidden
    }

    fn keep_hidden(&self) {
        #[cfg(target_os = "macos")]
        if self.hides_app() && !self.revealed.load(std::sync::atomic::Ordering::SeqCst) {
            if let Some(pid) = self.pid.filter(|p| crate::macos::is_hidden(*p) == Some(false)) {
                crate::macos::hide(pid);
            }
        }
    }

    async fn page_windows(&self) -> Vec<(String, Value)> {
        let Ok(t) = self.cdp.call(None, "Target.getTargets", json!({})).await else { return Vec::new() };
        let mut out = Vec::new();
        for info in t["targetInfos"].as_array().into_iter().flatten() {
            if info["type"] != "page" {
                continue;
            }
            let id = info["targetId"].as_str().unwrap_or_default().to_string();
            if let Ok(w) = self.cdp.call(None, "Browser.getWindowForTarget", json!({ "targetId": id })).await {
                out.push((id, w["windowId"].clone()));
            }
        }
        out
    }

    pub(crate) async fn reveal(&self, target_id: &str) -> Result<bool> {
        if !self.hides_app() {
            return Ok(false);
        }
        self.revealed.store(true, std::sync::atomic::Ordering::SeqCst);
        *self.shown.lock() = Some(target_id.to_string());
        for (id, wid) in self.page_windows().await {
            if id != target_id {
                let _ = self.cdp.call(None, "Browser.setWindowBounds", json!({ "windowId": wid, "bounds": { "windowState": "minimized" } })).await;
            }
        }
        #[cfg(target_os = "macos")]
        if let Some(pid) = self.pid {
            crate::macos::show(pid);
        }
        Ok(true)
    }

    pub(crate) async fn conceal(&self) -> Result<bool> {
        if !self.hides_app() {
            return Ok(false);
        }
        self.revealed.store(false, std::sync::atomic::Ordering::SeqCst);
        *self.shown.lock() = None;
        self.keep_hidden();
        Ok(true)
    }

    pub(crate) async fn page_closed(&self, target_id: &str) {
        let was_shown = self.shown.lock().as_deref() == Some(target_id);
        if was_shown {
            let _ = self.conceal().await;
        }
    }

    pub fn mode(&self) -> BrowserMode {
        self.opts.mode
    }

    pub fn is_alive(&self) -> bool {
        !self.cdp.is_closed()
    }

    /// Abre uma nova aba isolada em sua própria janela.
    pub async fn new_page(self: &Arc<Self>) -> Result<Arc<Page>> {
        self.new_page_in(None).await
    }

    /// Pega uma aba estacionada (ou abre uma nova). Reusar abas evita abrir e
    /// fechar janelas a cada busca, o que custa CPU. A aba volta para o
    /// estacionamento quando o `Lease` é descartado (inclusive num cancelamento).
    pub async fn lease(self: &Arc<Self>) -> Result<Lease> {
        loop {
            let pooled = self.pool.lock().pop();
            let Some(page) = pooled else { break };
            if page.alive().await {
                return Ok(Lease { browser: self.clone(), page: Some(page) });
            }
            page.close().await;
        }
        Ok(Lease { browser: self.clone(), page: Some(self.new_page().await?) })
    }

    /// Estaciona a aba em about:blank: os scripts da loja param e a aba
    /// ociosa não gasta CPU. A aba fica aberta (RAM) para o próximo uso.
    async fn give_back(&self, page: Arc<Page>) {
        if page.park().await.is_ok() {
            let mut pool = self.pool.lock();
            if pool.len() < POOL_MAX {
                pool.push(page);
                return;
            }
        }
        page.close().await;
    }

    /// Fecha as abas estacionadas. Chamado quando o Snipr fica ocioso.
    pub async fn drain_pool(&self) {
        let pages = std::mem::take(&mut *self.pool.lock());
        for p in pages {
            p.close().await;
        }
    }

    pub async fn new_isolated_page(self: &Arc<Self>) -> Result<Arc<Page>> {
        let ctx = self.cdp.call(None, "Target.createBrowserContext", json!({ "disposeOnDetach": true })).await?;
        let id = ctx["browserContextId"].as_str().ok_or_else(|| anyhow!("createBrowserContext sem id"))?.to_string();
        self.new_page_in(Some(id)).await
    }

    async fn new_page_in(self: &Arc<Self>, context: Option<String>) -> Result<Arc<Page>> {
        let mut params = json!({ "url": "about:blank" });
        if let Some(c) = &context {
            params["browserContextId"] = json!(c);
        }
        if self.opts.mode != BrowserMode::Headless {
            params["newWindow"] = json!(true);
            if self.opts.mode == BrowserMode::Hidden {
                if !self.hides_app() {
                    params["left"] = json!(HIDDEN_POS.0);
                    params["top"] = json!(HIDDEN_POS.1);
                }
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
        self.keep_hidden();
        let old = std::mem::take(&mut *self.startup.lock());
        for id in old {
            let _ = self.cdp.call(None, "Target.closeTarget", json!({ "targetId": id })).await;
        }
        let page = Arc::new(Page::new(self.clone(), session, target_id, context));
        page.init().await?;
        Ok(page)
    }

    pub async fn close(&self) {
        tracing::info!(pid = ?self.pid, "fechando navegador");
        let _ = self
            .cdp
            .call_timeout(None, "Browser.close", json!({}), Duration::from_secs(5))
            .await;
        if let Some(mut child) = self.child.lock().await.take() {
            let _ = tokio::time::timeout(Duration::from_secs(3), child.wait()).await;
            let _ = child.kill().await;
        } else if let Some(pid) = self.pid {
            let start = Instant::now();
            while pid_alive(pid) && start.elapsed() < Duration::from_secs(3) {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            kill_pid(pid);
        }
    }
}

/// Aba emprestada do estacionamento do `Browser`.
pub struct Lease {
    browser: Arc<Browser>,
    page: Option<Arc<Page>>,
}

impl Lease {
    pub fn page(&self) -> &Arc<Page> {
        self.page.as_ref().expect("aba emprestada")
    }
}

impl std::ops::Deref for Lease {
    type Target = Page;
    fn deref(&self) -> &Page {
        self.page()
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        let Some(page) = self.page.take() else { return };
        let browser = self.browser.clone();
        if let Ok(rt) = tokio::runtime::Handle::try_current() {
            rt.spawn(async move { browser.give_back(page).await });
        }
    }
}

impl Drop for Browser {
    fn drop(&mut self) {
        if self.child.get_mut().is_none() {
            if let Some(pid) = self.pid {
                kill_pid(pid);
            }
        }
    }
}

fn spawn(exe: &Path, args: &[String], hides_app: bool) -> Result<(Option<Child>, Option<u32>)> {
    #[cfg(target_os = "macos")]
    if hides_app {
        if let Some(bundle) = exe.ancestors().find(|p| p.extension().is_some_and(|e| e == "app")) {
            let ok = std::process::Command::new("open")
                .args(["-n", "-g", "-j", "-a"])
                .arg(bundle)
                .arg("--args")
                .args(args)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|s| s.success());
            if ok {
                return Ok((None, None));
            }
        }
    }
    let _ = hides_app;
    let child = Command::new(exe)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .with_context(|| format!("falha ao iniciar {}", exe.display()))?;
    let pid = child.id();
    Ok((Some(child), pid))
}

async fn page_targets(cdp: &Cdp) -> Vec<String> {
    let Ok(t) = cdp.call(None, "Target.getTargets", json!({})).await else { return Vec::new() };
    t["targetInfos"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|i| i["type"] == "page")
        .filter_map(|i| i["targetId"].as_str().map(String::from))
        .collect()
}

fn find_pid(profile: &Path) -> Option<u32> {
    if cfg!(windows) {
        return None;
    }
    let out = std::process::Command::new("ps").args(["-axww", "-o", "pid=,args="]).output().ok()?;
    let needle = format!("--user-data-dir={}", profile.display());
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .find(|l| l.contains(&needle) && !l.contains("--type="))
        .and_then(|l| l.split_whitespace().next()?.parse().ok())
}

fn pid_alive(pid: u32) -> bool {
    std::process::Command::new("kill").args(["-0", &pid.to_string()]).stderr(Stdio::null()).status().is_ok_and(|s| s.success())
}

fn kill_pid(pid: u32) {
    if cfg!(windows) {
        let _ = std::process::Command::new("taskkill").args(["/PID", &pid.to_string(), "/T", "/F"]).status();
    } else if pid_alive(pid) {
        let _ = std::process::Command::new("kill").arg(pid.to_string()).status();
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
fn build_ua_override(ua: &str, brand: &str) -> UaOverride {
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
            { "brand": brand, "version": major },
            { "brand": "Chromium", "version": major },
            { "brand": "Not_A Brand", "version": "24" }
        ],
        "fullVersionList": [
            { "brand": brand, "version": full },
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
