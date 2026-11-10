//! Uma aba controlada pelo agente.

use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

use anyhow::{anyhow, bail, Result};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::broadcast::error::RecvError;

use crate::{
    browser::{Browser, BrowserMode, HIDDEN_POS, WINDOW_SIZE},
    challenge::{self, ChallengeOutcome},
    human::{self, rnd},
    js,
};

#[derive(Debug, Clone, Serialize)]
pub struct NavInfo {
    pub url: String,
    pub title: String,
    pub challenge: ChallengeOutcome,
}

/// Card de anúncio lido de uma página de resultados.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Card {
    pub title: String,
    /// Preços atuais em reais, na ordem da página. Sem parcelas, descontos,
    /// fretes nem preços riscados.
    pub prices: Vec<String>,
    /// Preços riscados (antes do desconto).
    pub old: Vec<String>,
    /// Link do anúncio. Vazio quando o card não expõe link (ex.: Shopee).
    pub url: String,
    /// Texto restante do card (condição, vendedor, cidade, frete…).
    pub extra: String,
    pub intl: bool,
}

pub struct Page {
    browser: Arc<Browser>,
    pub session: String,
    pub target_id: String,
    world: Mutex<Option<i64>>,
    mouse: Mutex<(f64, f64)>,
    context: Option<String>,
    shown: AtomicBool,
}

const BLOCKED_PATTERNS: &[&str] = &[
    "*.png", "*.png?*", "*.jpg", "*.jpg?*", "*.jpeg", "*.jpeg?*", "*.gif", "*.gif?*", "*.webp", "*.webp?*",
    "*.avif", "*.avif?*", "*.ico", "*.woff", "*.woff2", "*.woff2?*", "*.ttf", "*.otf", "*.mp4", "*.webm",
    "*.m3u8", "*.mp3",
    // Redes de anúncio e rastreamento: pesam muito em RAM/CPU e não ajudam a achar preço.
    "*doubleclick.net*", "*googlesyndication.com*", "*googleadservices.com*", "*google-analytics.com*",
    "*googletagmanager.com*", "*adservice.google.*", "*taboola.com*", "*outbrain.com*", "*criteo.com*",
    "*criteo.net*", "*hotjar.com*", "*clarity.ms*", "*adsrvr.org*", "*amazon-adsystem.com*",
    "*rtbhouse.com*", "*tiktok.com/i18n/pixel*", "*analytics.tiktok.com*", "*connect.facebook.net*",
    "*bat.bing.com*", "*scorecardresearch.com*", "*yieldmo.com*", "*rubiconproject.com*", "*pubmatic.com*",
    "*smartadserver.com*", "*teads.tv*", "*adnxs.com*", "*rtb-*.linkedin.com*", "*ads.linkedin.com*",
];

impl Page {
    pub(crate) fn new(browser: Arc<Browser>, session: String, target_id: String, context: Option<String>) -> Self {
        Self {
            browser,
            session,
            target_id,
            context,
            shown: AtomicBool::new(false),
            world: Mutex::new(None),
            mouse: Mutex::new((
                rnd(300, 700) as f64,
                rnd(200, 450) as f64,
            )),
        }
    }

    pub(crate) async fn cmd(&self, method: &str, params: Value) -> Result<Value> {
        Ok(self.browser.cdp.call(Some(&self.session), method, params).await?)
    }

    pub(crate) async fn init(&self) -> Result<()> {
        self.cmd("Page.enable", json!({})).await?;
        let _ = self.cmd("Emulation.setFocusEmulationEnabled", json!({ "enabled": true })).await;
        if let Some(ua) = &self.browser.ua_override {
            let _ = self
                .cmd(
                    "Emulation.setUserAgentOverride",
                    json!({
                        "userAgent": ua.user_agent,
                        "acceptLanguage": "pt-BR,pt;q=0.9,en-US;q=0.8,en;q=0.7",
                        "userAgentMetadata": ua.metadata,
                    }),
                )
                .await;
        }
        if self.browser.opts.block_media {
            let _ = self.cmd("Network.enable", json!({})).await;
            let _ = self.cmd("Network.setBlockedURLs", json!({ "urls": BLOCKED_PATTERNS })).await;
        }
        Ok(())
    }

    async fn context(&self) -> Result<i64> {
        if let Some(c) = *self.world.lock() {
            return Ok(c);
        }
        let tree = self.cmd("Page.getFrameTree", json!({})).await?;
        let frame_id = tree["frameTree"]["frame"]["id"].as_str().ok_or_else(|| anyhow!("sem frame principal"))?.to_string();
        let r = self
            .cmd("Page.createIsolatedWorld", json!({ "frameId": frame_id, "worldName": "__snipr" }))
            .await?;
        let ctx = r["executionContextId"].as_i64().ok_or_else(|| anyhow!("isolated world sem contexto"))?;
        *self.world.lock() = Some(ctx);
        Ok(ctx)
    }

    /// Avalia JS no isolated world. Recria o mundo se a página navegou.
    pub async fn eval(&self, expr: &str) -> Result<Value> {
        let mut last_err = None;
        for _ in 0..2 {
            let ctx = self.context().await?;
            match self
                .cmd(
                    "Runtime.evaluate",
                    json!({ "expression": expr, "contextId": ctx, "returnByValue": true, "awaitPromise": true }),
                )
                .await
            {
                Ok(r) => {
                    if let Some(ex) = r.get("exceptionDetails") {
                        let msg = ex["exception"]["description"]
                            .as_str()
                            .or_else(|| ex["text"].as_str())
                            .unwrap_or("erro de script")
                            .to_string();
                        bail!("erro JS: {}", msg.lines().next().unwrap_or_default());
                    }
                    return Ok(r["result"]["value"].clone());
                }
                Err(e) => {
                    *self.world.lock() = None;
                    last_err = Some(e);
                }
            }
        }
        Err(last_err.unwrap_or_else(|| anyhow!("falha ao avaliar script")))
    }

    async fn call_fn(&self, func: &str, args: &[Value]) -> Result<Value> {
        let args = args.iter().map(|a| a.to_string()).collect::<Vec<_>>().join(",");
        self.eval(&format!("({func})({args})")).await
    }

    pub async fn wait_ready(&self, max: Duration) {
        let start = Instant::now();
        loop {
            match self.eval("document.readyState").await {
                Ok(v) if v == "complete" => break,
                Ok(v) if v == "interactive" && start.elapsed() > Duration::from_secs(4) => break,
                _ => {}
            }
            if start.elapsed() > max {
                break;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        tokio::time::sleep(Duration::from_millis(rnd(500, 1000))).await;
    }

    pub async fn goto(&self, url: &str) -> Result<NavInfo> {
        let nav = self.goto_once(url).await?;
        // Bloqueio "suave" de primeira visita (perfil ainda sem cookies, ex.: Amazon):
        // os cookies já foram gravados, então uma segunda tentativa costuma passar.
        let t = nav.title.to_lowercase();
        if ["algo deu errado", "something went wrong", "sorry! something", "page not available"]
            .iter()
            .any(|p| t.contains(p))
        {
            // Passa pela home (como uma pessoa faria) para ganhar os cookies de sessão.
            tracing::info!(url, "bloqueio suave detectado, passando pela home e tentando de novo");
            if let Ok(u) = url::Url::parse(url) {
                let home = format!("{}/", u.origin().ascii_serialization());
                tokio::time::sleep(Duration::from_millis(rnd(800, 1500))).await;
                let _ = self.goto_once(&home).await;
                tokio::time::sleep(Duration::from_millis(rnd(1500, 3000))).await;
            }
            return self.goto_once(url).await;
        }
        Ok(nav)
    }

    async fn goto_once(&self, url: &str) -> Result<NavInfo> {
        let mut rx = self.browser.cdp.subscribe();
        *self.world.lock() = None;
        let r = self
            .browser
            .cdp
            .call_timeout(Some(&self.session), "Page.navigate", json!({ "url": url }), Duration::from_secs(35))
            .await?;
        if let Some(err) = r["errorText"].as_str().filter(|s| !s.is_empty()) {
            bail!("falha ao abrir {url}: {err}");
        }
        let session = self.session.clone();
        let _ = tokio::time::timeout(Duration::from_secs(20), async move {
            loop {
                match rx.recv().await {
                    Ok(ev) if ev.session_id.as_deref() == Some(&session) && ev.method == "Page.domContentEventFired" => break,
                    Ok(_) | Err(RecvError::Lagged(_)) => continue,
                    Err(RecvError::Closed) => break,
                }
            }
        })
        .await;
        *self.world.lock() = None;
        self.wait_ready(Duration::from_secs(8)).await;
        self.wait_stable().await;
        let challenge = challenge::handle(self).await;
        self.info(challenge).await
    }

    /// Alguns sites recarregam a página sozinhos logo após abrir (ex.: ao
    /// gravar cookies na primeira visita). Esperamos o documento parar de mudar.
    async fn wait_stable(&self) {
        let origin = |v: anyhow::Result<Value>| v.ok().and_then(|v| v.as_f64()).unwrap_or(0.0);
        for _ in 0..4 {
            let a = origin(self.eval("performance.timeOrigin").await);
            tokio::time::sleep(Duration::from_millis(700)).await;
            let b = origin(self.eval("performance.timeOrigin").await);
            if a == b && a != 0.0 {
                return;
            }
            self.wait_ready(Duration::from_secs(8)).await;
        }
    }

    /// Garante que o documento atual já tem conteúdo antes de lê-lo.
    async fn ensure_loaded(&self) {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(8) {
            if let Ok(v) = self.eval("document.readyState !== 'loading' && !!document.body && document.body.childElementCount > 0").await {
                if v == Value::Bool(true) {
                    return;
                }
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }

    pub async fn info(&self, challenge: ChallengeOutcome) -> Result<NavInfo> {
        let v = self.eval("({ url: location.href, title: document.title })").await?;
        Ok(NavInfo {
            url: v["url"].as_str().unwrap_or_default().to_string(),
            title: v["title"].as_str().unwrap_or_default().to_string(),
            challenge,
        })
    }

    pub async fn url(&self) -> Result<String> {
        Ok(self.eval("location.href").await?.as_str().unwrap_or_default().to_string())
    }

    pub async fn snapshot(&self, max_items: usize, filter: Option<&str>) -> Result<String> {
        self.ensure_loaded().await;
        let v = self.call_fn(js::SNAPSHOT, &[json!(max_items), json!(filter)]).await?;
        Ok(v.as_str().unwrap_or_default().to_string())
    }

    pub async fn extract_cards(&self, max_cards: usize) -> Result<Vec<Card>> {
        self.ensure_loaded().await;
        let read = || async {
            let v = self.call_fn(js::EXTRACT_LISTINGS, &[json!(max_cards)]).await?;
            Ok::<_, anyhow::Error>(serde_json::from_str::<Vec<Card>>(v.as_str().unwrap_or("[]"))?)
        };
        let cards = read().await?;
        if !cards.is_empty() {
            return Ok(cards);
        }
        // Conteúdo renderizado por JS pode chegar um pouco depois.
        tokio::time::sleep(Duration::from_millis(1500)).await;
        read().await
    }

    pub async fn read_text(&self, max_chars: usize) -> Result<String> {
        self.ensure_loaded().await;
        let v = self.call_fn(js::READ_TEXT, &[json!(max_chars)]).await?;
        Ok(v.as_str().unwrap_or_default().to_string())
    }

    /// Abre seções recolhidas (acordeões, "ver mais", ficha técnica) com
    /// cliques reais e lê o texto da página.
    pub async fn read_details(&self, max_chars: usize) -> Result<String> {
        self.ensure_loaded().await;
        let targets = self.call_fn(js::EXPAND_TARGETS, &[json!(4)]).await.unwrap_or_default();
        for r in targets.as_array().into_iter().flatten().filter_map(Value::as_u64) {
            match self.click_ref(r as u32).await {
                // Um clique que navegou para outra página encerra a expansão.
                Ok(msg) if msg.starts_with("Clicado. Navegou") || msg.starts_with("Link aberto") => break,
                _ => tokio::time::sleep(Duration::from_millis(rnd(250, 500))).await,
            }
        }
        self.read_text(max_chars).await
    }

    /// Cookies que o navegador mandaria para estas URLs, no formato do
    /// cabeçalho `Cookie`.
    pub async fn cookie_header(&self, urls: &[&str]) -> Result<String> {
        let v = self.cmd("Network.getCookies", json!({ "urls": urls })).await?;
        Ok(v["cookies"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|c| Some(format!("{}={}", c["name"].as_str()?, c["value"].as_str()?)))
            .collect::<Vec<_>>()
            .join("; "))
    }

    /// Para os scripts da página (about:blank) e oculta a janela, se estava visível.
    pub(crate) async fn park(&self) -> Result<()> {
        if self.shown.load(Ordering::SeqCst) {
            let _ = self.set_visible(false).await;
        }
        *self.world.lock() = None;
        self.browser
            .cdp
            .call_timeout(Some(&self.session), "Page.navigate", json!({ "url": "about:blank" }), Duration::from_secs(10))
            .await?;
        *self.world.lock() = None;
        Ok(())
    }

    pub(crate) async fn alive(&self) -> bool {
        self.browser
            .cdp
            .call_timeout(None, "Target.getTargetInfo", json!({ "targetId": self.target_id }), Duration::from_secs(5))
            .await
            .is_ok()
    }

    pub async fn detect_challenge(&self) -> Option<String> {
        self.eval(js::DETECT_CHALLENGE).await.ok().and_then(|v| v.as_str().map(String::from))
    }

    async fn locate(&self, r: u32) -> Result<Value> {
        let v = self.call_fn(js::LOCATE_REF, &[json!(r), json!(true)]).await?;
        if v.is_null() {
            bail!("referência [{r}] não existe mais (a página mudou). Chame snapshot de novo.");
        }
        tokio::time::sleep(Duration::from_millis(rnd(120, 260))).await;
        let v = self.call_fn(js::LOCATE_REF, &[json!(r), json!(false)]).await?;
        if v.is_null() {
            bail!("referência [{r}] sumiu da página. Chame snapshot de novo.");
        }
        Ok(v)
    }

    /// Clique "humano" num elemento do último snapshot.
    pub async fn click_ref(&self, r: u32) -> Result<String> {
        let loc = self.locate(r).await?;
        if loc["blank"].as_bool() == Some(true) {
            if let Some(href) = loc["href"].as_str() {
                // Link que abriria nova aba: seguimos nesta mesma aba.
                let nav = self.goto(href).await?;
                return Ok(format!("Link aberto: {} — {}", nav.url, nav.title));
            }
        }
        let (x, y, w, h) = (
            loc["x"].as_f64().unwrap_or(0.0),
            loc["y"].as_f64().unwrap_or(0.0),
            loc["w"].as_f64().unwrap_or(4.0),
            loc["h"].as_f64().unwrap_or(4.0),
        );
        let jx = (w / 4.0).min(8.0);
        let jy = (h / 4.0).min(4.0);
        let tx = x + human::jitter(jx);
        let ty = y + human::jitter(jy);
        let before = self.url().await.unwrap_or_default();
        self.click_at(tx, ty).await?;
        tokio::time::sleep(Duration::from_millis(rnd(700, 1200))).await;
        let after = self.url().await.unwrap_or_default();
        if after != before {
            *self.world.lock() = None;
            self.wait_ready(Duration::from_secs(10)).await;
            let ch = challenge::handle(self).await;
            let nav = self.info(ch).await?;
            return Ok(format!("Clicado. Navegou para {} — {}", nav.url, nav.title));
        }
        Ok("Clicado.".into())
    }

    pub async fn click_at(&self, x: f64, y: f64) -> Result<()> {
        self.move_mouse(x, y).await?;
        tokio::time::sleep(Duration::from_millis(rnd(40, 130))).await;
        let base = json!({ "x": x, "y": y, "button": "left", "clickCount": 1 });
        let mut down = base.clone();
        down["type"] = json!("mousePressed");
        self.cmd("Input.dispatchMouseEvent", down).await?;
        tokio::time::sleep(Duration::from_millis(rnd(55, 140))).await;
        let mut up = base;
        up["type"] = json!("mouseReleased");
        self.cmd("Input.dispatchMouseEvent", up).await?;
        Ok(())
    }

    async fn move_mouse(&self, x: f64, y: f64) -> Result<()> {
        let from = *self.mouse.lock();
        for (px, py, delay) in human::mouse_path(from, (x, y)) {
            self.cmd("Input.dispatchMouseEvent", json!({ "type": "mouseMoved", "x": px, "y": py }))
                .await?;
            tokio::time::sleep(Duration::from_millis(delay)).await;
        }
        *self.mouse.lock() = (x, y);
        Ok(())
    }

    /// Foca o campo, seleciona o conteúdo existente e digita com cadência humana.
    pub async fn type_into(&self, r: u32, text: &str, submit: bool) -> Result<()> {
        let loc = self.locate(r).await?;
        let x = loc["x"].as_f64().unwrap_or(0.0) + human::jitter(6.0);
        let y = loc["y"].as_f64().unwrap_or(0.0) + human::jitter(2.0);
        self.click_at(x, y).await?;
        tokio::time::sleep(Duration::from_millis(rnd(120, 300))).await;
        let _ = self.call_fn(js::SELECT_ALL_IN_REF, &[json!(r)]).await;
        self.type_text(text).await?;
        if submit {
            tokio::time::sleep(Duration::from_millis(rnd(150, 400))).await;
            self.press_enter().await?;
        }
        Ok(())
    }

    pub async fn type_text(&self, text: &str) -> Result<()> {
        for ch in text.chars() {
            let s = ch.to_string();
            self.cmd(
                "Input.dispatchKeyEvent",
                json!({ "type": "keyDown", "text": s, "unmodifiedText": s, "key": s }),
            )
            .await?;
            self.cmd("Input.dispatchKeyEvent", json!({ "type": "keyUp", "key": s })).await?;
            tokio::time::sleep(Duration::from_millis(human::key_delay())).await;
        }
        Ok(())
    }

    pub async fn press_enter(&self) -> Result<()> {
        let before = self.url().await.unwrap_or_default();
        let k = json!({ "key": "Enter", "code": "Enter", "windowsVirtualKeyCode": 13, "nativeVirtualKeyCode": 13 });
        let mut down = k.clone();
        down["type"] = json!("keyDown");
        down["text"] = json!("\r");
        self.cmd("Input.dispatchKeyEvent", down).await?;
        let mut up = k;
        up["type"] = json!("keyUp");
        self.cmd("Input.dispatchKeyEvent", up).await?;
        tokio::time::sleep(Duration::from_millis(rnd(900, 1400))).await;
        if self.url().await.unwrap_or_default() != before {
            *self.world.lock() = None;
            self.wait_ready(Duration::from_secs(10)).await;
            challenge::handle(self).await;
        }
        Ok(())
    }

    pub async fn scroll(&self, down: bool) -> Result<()> {
        let (mut x, mut y) = *self.mouse.lock();
        if x < 50.0 || y < 50.0 {
            x = rnd(400, 800) as f64;
            y = rnd(300, 500) as f64;
            self.move_mouse(x, y).await?;
        }
        let ticks = rnd(3, 6);
        for _ in 0..ticks {
            let dy = rnd(100, 180) as f64 * if down { 1.0 } else { -1.0 };
            self.cmd(
                "Input.dispatchMouseEvent",
                json!({ "type": "mouseWheel", "x": x, "y": y, "deltaX": 0, "deltaY": dy }),
            )
            .await?;
            tokio::time::sleep(Duration::from_millis(rnd(40, 110))).await;
        }
        tokio::time::sleep(Duration::from_millis(rnd(500, 900))).await;
        Ok(())
    }

    pub async fn go_back(&self) -> Result<NavInfo> {
        let h = self.cmd("Page.getNavigationHistory", json!({})).await?;
        let idx = h["currentIndex"].as_i64().unwrap_or(0);
        if idx <= 0 {
            bail!("não há página anterior");
        }
        let entry = &h["entries"][(idx - 1) as usize];
        *self.world.lock() = None;
        self.cmd("Page.navigateToHistoryEntry", json!({ "entryId": entry["id"] })).await?;
        tokio::time::sleep(Duration::from_millis(600)).await;
        *self.world.lock() = None;
        self.wait_ready(Duration::from_secs(10)).await;
        self.info(ChallengeOutcome::None).await
    }

    /// Mostra/oculta a janela desta aba (para login manual ou desafio).
    pub async fn set_visible(&self, visible: bool) -> Result<()> {
        if self.browser.mode() == BrowserMode::Headless {
            bail!("o navegador está em modo headless; mude para 'oculto' nos ajustes para poder mostrar a janela");
        }
        self.shown.store(visible, Ordering::SeqCst);
        if !visible && self.browser.conceal().await? {
            return Ok(());
        }
        if visible {
            self.browser.reveal(&self.target_id).await?;
        }
        let w = self
            .browser
            .cdp
            .call(None, "Browser.getWindowForTarget", json!({ "targetId": self.target_id }))
            .await?;
        let wid = w["windowId"].clone();
        let bounds = if visible {
            json!({ "left": 80, "top": 60, "width": 1280, "height": 860, "windowState": "normal" })
        } else if self.browser.mode() == BrowserMode::Hidden {
            json!({ "left": HIDDEN_POS.0, "top": HIDDEN_POS.1, "width": WINDOW_SIZE.0, "height": WINDOW_SIZE.1, "windowState": "normal" })
        } else {
            return Ok(());
        };
        self.browser
            .cdp
            .call(None, "Browser.setWindowBounds", json!({ "windowId": wid, "bounds": bounds }))
            .await?;
        if visible {
            let _ = self.cmd("Page.bringToFront", json!({})).await;
        }
        Ok(())
    }

    pub async fn close(&self) {
        let _ = self
            .browser
            .cdp
            .call_timeout(None, "Target.closeTarget", json!({ "targetId": self.target_id }), Duration::from_secs(5))
            .await;
        self.browser.page_closed(&self.target_id).await;
        if let Some(ctx) = &self.context {
            let _ = self
                .browser
                .cdp
                .call_timeout(None, "Target.disposeBrowserContext", json!({ "browserContextId": ctx }), Duration::from_secs(5))
                .await;
        }
    }
}
