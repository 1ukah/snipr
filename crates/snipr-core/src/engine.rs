//! O orquestrador. É a única API que um frontend (GUI, bot, servidor) usa.

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, AtomicUsize, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

use anyhow::{anyhow, bail, Result};
use parking_lot::{Mutex, RwLock};
use serde_json::Value;
use snipr_browser::{detect_browsers, Browser, BrowserMode, LaunchOptions, Page};
use snipr_llm::{
    anthropic::{self, AnthropicConfig, AnthropicSession, DEFAULT_MODEL},
    openai::{self, OpenAiConfig, OpenAiSession},
    ChatSession,
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::{
    cli::{self, CliKind, CliRun, CliStatus},
    config::{AiSettings, AuthMode, Paths, Provider, SessionState, Settings},
    fetch::Fetcher,
    mcp::McpServer,
    models, prompts, secrets,
    sites::{self, Site, StageInfo, Suggestion},
    tools::{HumanGates, ToolBox},
    types::{BrowserStatus, DetectedBrowser, Listing, SearchEvent, SearchPlan, SearchRequest, Usage},
};

pub type EventSink = mpsc::UnboundedSender<SearchEvent>;

const ANTHROPIC_URL: &str = "https://api.anthropic.com";

pub struct Engine {
    pub(crate) paths: Paths,
    settings: RwLock<Settings>,
    browser: tokio::sync::Mutex<Option<Arc<Browser>>>,
    pub(crate) mcp: Arc<McpServer>,
    pub(crate) gates: Arc<HumanGates>,
    jobs: Mutex<HashMap<String, CancellationToken>>,
    active: AtomicUsize,
    generation: AtomicU64,
    login_page: tokio::sync::Mutex<Option<(String, Arc<Page>)>>,
    pub(crate) fetcher: Fetcher,
    /// Modelos listados por provedor (para escolher o modelo rápido).
    model_lists: Mutex<HashMap<String, Vec<String>>>,
    /// Runtime onde o Engine foi criado. Os métodos síncronos (ex.: `start_search`)
    /// podem ser chamados de qualquer thread — inclusive a thread da interface,
    /// que não tem runtime Tokio — e todo trabalho assíncrono vai para cá.
    rt: tokio::runtime::Handle,
}

pub(crate) enum Backend {
    Cli(CliRun),
    Chat { provider: Provider, base_url: String, api_key: Option<String>, model: String, effort: Option<String> },
}

/// Qual modelo usar numa chamada.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Tier {
    /// Plano e filtro: chamadas curtas de classificação.
    Fast,
    /// Agent e segunda tentativa do filtro.
    Main,
}

impl Engine {
    pub async fn new() -> Result<Arc<Self>> {
        let paths = Paths::resolve()?;
        // .env opcional: na pasta atual e na pasta de configuração.
        let _ = dotenvy::dotenv();
        if let Some(dir) = paths.config_file.parent() {
            let _ = dotenvy::from_path(dir.join(".env"));
        }
        Self::with_paths(paths).await
    }

    pub(crate) async fn with_paths(paths: Paths) -> Result<Arc<Self>> {
        let settings = paths.load_settings();
        let mcp = McpServer::start().await?;
        Ok(Arc::new(Self {
            paths,
            settings: RwLock::new(settings),
            browser: tokio::sync::Mutex::new(None),
            mcp,
            gates: Arc::new(HumanGates::default()),
            jobs: Mutex::new(HashMap::new()),
            active: AtomicUsize::new(0),
            generation: AtomicU64::new(0),
            login_page: tokio::sync::Mutex::new(None),
            fetcher: Fetcher::new()?,
            model_lists: Mutex::new(HashMap::new()),
            rt: tokio::runtime::Handle::current(),
        }))
    }

    pub fn settings(&self) -> Settings {
        self.settings.read().clone()
    }

    pub async fn save_settings(&self, mut s: Settings) -> Result<()> {
        s.max_parallel = s.max_parallel.clamp(1, 8);
        s.max_steps = s.max_steps.clamp(5, 60);
        let browser_changed = {
            let old = self.settings.read();
            old.browser_mode != s.browser_mode
                || old.block_media != s.block_media
                || old.low_memory != s.low_memory
                || old.chrome_path != s.chrome_path
        };
        self.store(s)?;
        if browser_changed && self.active.load(Ordering::SeqCst) == 0 {
            self.close_browser().await;
        }
        Ok(())
    }

    fn store(&self, s: Settings) -> Result<()> {
        self.paths.save_settings(&s)?;
        *self.settings.write() = s;
        Ok(())
    }

    pub(crate) fn update(&self, f: impl FnOnce(&mut Settings)) -> Result<Settings> {
        let mut s = self.settings();
        f(&mut s);
        self.store(s.clone())?;
        Ok(s)
    }

    pub fn sites(&self) -> Vec<Site> {
        self.settings.read().all_sites()
    }

    pub fn suggestions(&self) -> Vec<Suggestion> {
        let s = self.settings.read();
        let mut out: Vec<Suggestion> = sites::default_sites()
            .into_iter()
            .filter(|d| s.removed_sites.contains(&d.id))
            .map(|d| Suggestion { url: d.home(), domain: d.domain.clone(), name: d.name })
            .collect();
        let taken: Vec<String> = s.all_sites().into_iter().map(|x| x.domain.trim_start_matches("www.").to_string()).collect();
        out.extend(sites::suggestions().into_iter().filter(|x| !taken.iter().any(|t| x.domain.starts_with(t.as_str()))));
        out
    }

    pub fn mcp_port(&self) -> u16 {
        self.mcp.port
    }

    // ---------- segredos (somente escrita a partir de fora) ----------

    pub fn set_secret(&self, name: &str, value: &str) -> Result<()> {
        secrets::set(name, value)
    }

    pub fn has_secret(&self, name: &str) -> bool {
        secrets::has(name)
    }

    pub fn delete_secret(&self, name: &str) -> Result<()> {
        secrets::delete(name)
    }

    // ---------- IA ----------

    pub async fn ai_status(&self) -> Vec<CliStatus> {
        let custom = self.settings().ai.cli_path;
        let mut out = Vec::new();
        for p in [Provider::Anthropic, Provider::Openai, Provider::Google] {
            if let Some(kind) = CliKind::for_provider(p) {
                let path = if self.settings().ai.login_provider == p { custom.as_str() } else { "" };
                out.push(cli::status(kind, p, path).await);
            }
        }
        out
    }

    pub fn open_ai_login(&self, provider: Provider) -> Result<()> {
        let kind = CliKind::for_provider(provider).ok_or_else(|| anyhow!("{} não tem login por conta.", provider.label()))?;
        let s = self.settings();
        let custom = if s.ai.login_provider == provider { s.ai.cli_path } else { String::new() };
        cli::open_login(kind, &custom)
    }

    pub async fn list_models(&self, provider: Provider, endpoint: &str) -> Result<Vec<String>> {
        let key = secrets::reveal(&secrets::api_key_name(provider));
        if provider == Provider::Anthropic {
            let key = key.ok_or_else(|| anyhow!("Salve a API key da Anthropic primeiro."))?;
            return anthropic::list_models(&key, ANTHROPIC_URL).await;
        }
        let base = match provider.compat_base_url() {
            Some(b) => b.to_string(),
            None if !endpoint.trim().is_empty() => endpoint.trim().to_string(),
            None => bail!("Informe o endpoint."),
        };
        if provider.needs_key() && provider != Provider::Custom && key.is_none() {
            bail!("Salve a API key da {} primeiro.", provider.label());
        }
        openai::list_models(&base, key.as_deref()).await
    }

    /// Modelos do CLI de login do provedor (Claude Code, Codex, Antigravity).
    pub async fn cli_models(&self, provider: Provider) -> Result<Vec<String>> {
        let kind = CliKind::for_provider(provider).ok_or_else(|| anyhow!("{} não tem login por conta.", provider.label()))?;
        let s = self.settings();
        let custom = if s.ai.login_provider == provider { s.ai.cli_path } else { String::new() };
        cli::models(kind, &custom).await
    }

    /// Modelo e esforço de cada tipo de chamada.
    pub(crate) async fn model_for(&self, ai: &AiSettings, tier: Tier) -> (String, Option<String>) {
        let effort = Some(ai.effort.trim().to_string()).filter(|e| !e.is_empty());
        let main = ai.model.trim().to_string();
        if tier == Tier::Main {
            return (main, effort);
        }
        if !ai.fast_model.trim().is_empty() {
            return (ai.fast_model.trim().to_string(), None);
        }
        let provider = ai.provider();
        match ai.mode {
            // O Claude Code aceita o apelido da família e resolve para a versão atual.
            // Esforço baixo: a resposta é uma lista de ids, sem raciocínio longo.
            AuthMode::Login | AuthMode::Oauth if provider == Provider::Anthropic => ("haiku".into(), Some("low".into())),
            // Codex e Antigravity: o CLI escolhe o modelo da conta.
            AuthMode::Login | AuthMode::Oauth => (main, None),
            AuthMode::ApiKey => {
                let key = format!("{}|{}", provider.id(), ai.endpoint.trim());
                let cached = self.model_lists.lock().get(&key).cloned();
                let list = match cached {
                    Some(l) => l,
                    None => {
                        let l = self.list_models(provider, &ai.endpoint).await.unwrap_or_default();
                        if !l.is_empty() {
                            self.model_lists.lock().insert(key, l.clone());
                        }
                        l
                    }
                };
                (models::pick_fast(provider, &list).unwrap_or(main), None)
            }
        }
    }

    pub(crate) fn backend(&self, ai: &AiSettings, dir: PathBuf, model: &str, effort: Option<String>) -> Result<Backend> {
        let model = model.trim().to_string();
        match ai.mode {
            AuthMode::Login => {
                let p = ai.login_provider;
                let kind = CliKind::for_provider(p).ok_or_else(|| anyhow!("O login por conta funciona com Anthropic, OpenAI e Google."))?;
                let binary = cli::find(kind, &ai.cli_path)
                    .ok_or_else(|| anyhow!("O {} não está instalado. Instale o {} para usar o login da {}.", kind.label(), kind.label(), p.label()))?;
                Ok(Backend::Cli(CliRun { kind, binary, model: Some(model).filter(|m| !m.is_empty()), effort, oauth_token: None, dir }))
            }
            AuthMode::Oauth => {
                if ai.oauth_provider != Provider::Anthropic {
                    bail!("O token OAuth funciona somente com a Anthropic.");
                }
                let token = secrets::reveal(&secrets::oauth_name(Provider::Anthropic))
                    .ok_or_else(|| anyhow!("Salve o token OAuth em Ajustes > IA."))?;
                let binary = cli::find(CliKind::Claude, &ai.cli_path)
                    .ok_or_else(|| anyhow!("O token OAuth da Anthropic requer o Claude Code instalado."))?;
                Ok(Backend::Cli(CliRun {
                    kind: CliKind::Claude,
                    binary,
                    model: Some(model).filter(|m| !m.is_empty()),
                    effort,
                    oauth_token: Some(token),
                    dir,
                }))
            }
            AuthMode::ApiKey => {
                let p = ai.key_provider;
                let api_key = secrets::reveal(&secrets::api_key_name(p));
                if p.needs_key() && p != Provider::Custom && api_key.is_none() {
                    bail!("Salve a API key da {} em Ajustes > IA.", p.label());
                }
                let base_url = match p {
                    Provider::Anthropic => ANTHROPIC_URL.to_string(),
                    Provider::Custom if ai.endpoint.trim().is_empty() => bail!("Informe o endpoint em Ajustes > IA."),
                    Provider::Custom => ai.endpoint.trim().to_string(),
                    other => other.compat_base_url().unwrap_or_default().to_string(),
                };
                let model = if model.is_empty() && p == Provider::Anthropic { DEFAULT_MODEL.to_string() } else { model };
                if model.is_empty() {
                    bail!("Escolha um modelo em Ajustes > IA.");
                }
                Ok(Backend::Chat { provider: p, base_url, api_key, model, effort })
            }
        }
    }

    pub(crate) fn chat(backend: &Backend, system: &str, with_tools: bool) -> Option<Box<dyn ChatSession>> {
        let Backend::Chat { provider, base_url, api_key, model, effort } = backend else { return None };
        let specs = if with_tools { ToolBox::specs() } else { Vec::new() };
        Some(if *provider == Provider::Anthropic {
            Box::new(AnthropicSession::new(
                AnthropicConfig {
                    api_key: api_key.clone().unwrap_or_default(),
                    model: model.clone(),
                    effort: effort.clone(),
                    base_url: base_url.clone(),
                },
                system.to_string(),
                &specs,
            ))
        } else {
            Box::new(OpenAiSession::new(
                OpenAiConfig { base_url: base_url.clone(), api_key: api_key.clone(), model: model.clone(), effort: effort.clone() },
                system.to_string(),
                &specs,
            ))
        })
    }

    /// Uma pergunta sem ferramentas, com o modelo do tipo pedido.
    pub(crate) async fn ask(&self, ai: &AiSettings, tier: Tier, system: &str, prompt: String, dir: &str, cancel: CancellationToken) -> Result<(String, Usage)> {
        let (model, effort) = self.model_for(ai, tier).await;
        let backend = self.backend(ai, self.paths.work_dir.join(dir), &model, effort)?;
        match backend {
            Backend::Cli(run) => cli::run_text(run, system, prompt, cancel).await,
            ref chat => {
                let mut session = Self::chat(chat, system, false).ok_or_else(|| anyhow!("sem sessão"))?;
                let turn = tokio::select! {
                    r = session.user(prompt) => r?,
                    _ = cancel.cancelled() => bail!("cancelado"),
                };
                let usage = Usage::from_turn(&turn);
                Ok((turn.text, usage))
            }
        }
    }

    pub async fn plan(&self, query: &str, cancel: CancellationToken) -> SearchPlan {
        let ai = self.settings().ai;
        self.plan_with(&ai, query, cancel).await
    }

    pub async fn plan_with(&self, ai: &AiSettings, query: &str, cancel: CancellationToken) -> SearchPlan {
        self.plan_usage(ai, query, cancel).await.0
    }

    pub(crate) async fn plan_usage(&self, ai: &AiSettings, query: &str, cancel: CancellationToken) -> (SearchPlan, Usage) {
        let local = prompts::local_plan(query);
        let run = self.ask(ai, Tier::Fast, prompts::PLANNER, prompts::planner_input(query), "planner", cancel);
        match tokio::time::timeout(Duration::from_secs(120), run).await {
            Ok(Ok((text, usage))) => match prompts::parse_plan(&text, query) {
                Some(mut plan) => {
                    plan.max_price = plan.max_price.or(local.max_price);
                    plan.condition = plan.condition.or(local.condition);
                    (plan, usage)
                }
                None => {
                    tracing::warn!("plano de busca indisponível: resposta sem plano");
                    (local, usage)
                }
            },
            Ok(Err(e)) => {
                tracing::warn!("plano de busca indisponível: {e:#}");
                (local, Usage::default())
            }
            Err(_) => {
                tracing::warn!("plano de busca demorou demais");
                (local, Usage::default())
            }
        }
    }

    // ---------- lojas ----------

    pub async fn add_site(self: &Arc<Self>, url: &str, name: Option<String>, manual: bool) -> Result<Site> {
        let raw = url.trim();
        let full = if raw.starts_with("http://") || raw.starts_with("https://") { raw.to_string() } else { format!("https://{raw}") };
        let parsed = url::Url::parse(&full).map_err(|_| anyhow!("Digite uma URL válida. Exemplo: https://www.loja.com.br"))?;
        let host = parsed.host_str().unwrap_or_default().trim_start_matches("www.").to_string();
        if !host.contains('.') {
            bail!("Digite uma URL válida. Exemplo: https://www.loja.com.br");
        }
        if let Some(d) = sites::default_sites().into_iter().find(|d| d.matches_host(&host)) {
            if self.settings().removed_sites.contains(&d.id) {
                return self.restore_site(&d.id);
            }
        }
        if self.sites().iter().any(|s| s.matches_host(&host)) {
            bail!("Esta loja já está na lista.");
        }
        let probe = self.probe_site(&full).await;
        let (requires_login, form_search, title) = match probe {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!("não foi possível verificar {full}: {e:#}");
                (false, String::new(), String::new())
            }
        };
        let fallback = host.split('.').next().unwrap_or(&host).to_string();
        let name = name
            .filter(|n| !n.trim().is_empty())
            .or_else(|| Some(title).filter(|t| !t.is_empty() && t.chars().count() <= 40))
            .unwrap_or_else(|| fallback[..1].to_uppercase() + &fallback[1..]);
        let domain = sites::display_domain(&full);
        let mut site = Site {
            id: sites::site_id_for(&domain),
            name,
            domain: host.clone(),
            search_url: String::new(),
            home_url: full,
            login_url: None,
            requires_login,
            sells: String::new(),
            tips: String::new(),
            custom: true,
            manual,
            stage: None,
            api: None,
            json_url: String::new(),
        };
        // Lojas com login são lidas no Chrome, com a sessão do usuário.
        let info = if requires_login { None } else { self.detect_stage(&site, &form_search).await.ok() };
        if let Some(i) = &info {
            site.search_url = i.search_url.clone();
            site.stage = Some(i.stage);
            site.api = i.api;
            site.json_url = i.json_url.clone();
        } else {
            site.search_url = form_search;
        }
        let added = site.clone();
        self.update(|s| {
            if !s.enabled_sites.contains(&site.id) && !site.requires_login {
                s.enabled_sites.push(site.id.clone());
            }
            if let Some(i) = info {
                s.site_stages.insert(site.id.clone(), i);
            }
            s.custom_sites.push(site);
        })?;
        Ok(added)
    }

    pub fn restore_site(&self, id: &str) -> Result<Site> {
        let site = sites::default_sites().into_iter().find(|d| d.id == id).ok_or_else(|| anyhow!("loja desconhecida"))?;
        let id = id.to_string();
        self.update(|s| {
            s.removed_sites.retain(|x| x != &id);
            if !site.requires_login && !s.enabled_sites.contains(&id) {
                s.enabled_sites.push(id.clone());
            }
        })?;
        Ok(site)
    }

    pub fn remove_site(&self, id: &str) -> Result<()> {
        let id = id.to_string();
        self.update(|s| {
            if sites::default_sites().iter().any(|d| d.id == id) && !s.removed_sites.contains(&id) {
                s.removed_sites.push(id.clone());
            }
            s.custom_sites.retain(|c| c.id != id);
            s.enabled_sites.retain(|x| x != &id);
            s.sessions.remove(&id);
            s.login_overrides.remove(&id);
            s.site_stages.remove(&id);
        })?;
        Ok(())
    }

    /// Abre a home numa aba sem cookies: diz se ela pede login e acha o
    /// formulário de busca (validado depois em `detect_stage`).
    async fn probe_site(self: &Arc<Self>, url: &str) -> Result<(bool, String, String)> {
        const JS: &str = r#"(() => {
            const path = (location.pathname + location.search).toLowerCase();
            const wall = /(^|[\/_.?=-])(login|signin|sign-in|entrar|acesso|auth)([\/_.?=&-]|$)/.test(path)
                || [...document.querySelectorAll('input[type=password]')].some((e) => { const r = e.getBoundingClientRect(); return r.width > 0 && r.height > 0; });
            const og = document.querySelector('meta[property="og:site_name"]')?.content || '';
            const title = (og || document.title || '').split(/[|\-–—:]/)[0].trim();
            let search = '';
            const inputs = [...document.querySelectorAll('input[type=search], input[name=q], input[name=s], input[name=query], input[name=busca], input[name=keyword], input[name=term], input[name=ft], input[name=text], input[name=palavra_busca], input[name=search]')];
            for (const i of inputs) {
                const form = i.closest('form');
                if (!form || !i.name) continue;
                const action = new URL(form.getAttribute('action') || location.pathname, location.href);
                if ((form.method || 'get').toLowerCase() !== 'get' || action.origin !== location.origin) continue;
                for (const h of form.querySelectorAll('input[type=hidden][name]')) action.searchParams.set(h.name, h.value);
                action.searchParams.set(i.name, '__Q__');
                search = action.toString().replace('__Q__', '{q}');
                break;
            }
            return { wall, title, search };
        })()"#;
        self.active.fetch_add(1, Ordering::SeqCst);
        let r: Result<(bool, String, String)> = async {
            let browser = self.browser().await?;
            let page = browser.new_isolated_page().await?;
            let out = async {
                page.goto(url).await?;
                let v: Value = page.eval(JS).await?;
                Ok::<_, anyhow::Error>((
                    v["wall"].as_bool().unwrap_or(false),
                    v["search"].as_str().unwrap_or_default().to_string(),
                    v["title"].as_str().unwrap_or_default().to_string(),
                ))
            }
            .await;
            page.close().await;
            out
        }
        .await;
        self.release();
        r
    }

    /// Verifica de novo se a loja pede login e qual etapa lê os resultados.
    pub async fn check_site(self: &Arc<Self>, id: &str) -> Result<Site> {
        let site = self.sites().into_iter().find(|s| s.id == id).ok_or_else(|| anyhow!("loja desconhecida"))?;
        let (requires_login, form_search, _) = self.probe_site(&site.home()).await?;
        let info = if requires_login && site.custom { None } else { self.detect_stage(&site, &form_search).await.ok() };
        let id = site.id.clone();
        let s = self.update(|s| {
            if let Some(c) = s.custom_sites.iter_mut().find(|c| c.id == id) {
                c.requires_login = requires_login;
                match &info {
                    // A verificação manda: URL vazia = sem busca legível (o agent busca).
                    Some(i) => c.search_url = i.search_url.clone(),
                    None if c.search_url.is_empty() && !form_search.is_empty() => c.search_url = form_search.clone(),
                    None => {}
                }
                s.login_overrides.remove(&id);
            } else {
                s.login_overrides.insert(id.clone(), requires_login);
            }
            if let Some(i) = info {
                s.site_stages.insert(id.clone(), i);
            }
        })?;
        s.all_sites().into_iter().find(|x| x.id == site.id).ok_or_else(|| anyhow!("loja desconhecida"))
    }

    /// Grava a etapa que funcionou numa busca (sobe quando a loja bloqueia,
    /// desce quando uma etapa mais leve volta a funcionar).
    pub(crate) fn record_stage(&self, site: &Site, stage: sites::Stage) {
        let now = unix_now();
        let id = site.id.clone();
        let (api, url, json_url) = (site.api, site.search_url.clone(), site.json_url.clone());
        if let Err(e) = self.update(|s| {
            let e = s.site_stages.entry(id).or_insert(StageInfo { stage, api, search_url: url, json_url, checked_at: now });
            e.stage = stage;
            e.checked_at = now;
        }) {
            tracing::warn!("não foi possível salvar a etapa de {}: {e:#}", site.id);
        }
    }

    pub fn set_session(&self, site_id: &str, ok: bool) -> Result<()> {
        let id = site_id.to_string();
        self.update(|s| {
            let was_ok = s.sessions.get(&id) == Some(&SessionState::Ok);
            if ok {
                s.sessions.insert(id.clone(), SessionState::Ok);
            } else if was_ok {
                s.sessions.insert(id.clone(), SessionState::Expired);
            }
            if !ok {
                if let Some(c) = s.custom_sites.iter_mut().find(|c| c.id == id) {
                    c.requires_login = true;
                }
            }
        })?;
        Ok(())
    }

    // ---------- navegador ----------

    /// Navegadores compatíveis encontrados e qual será usado.
    pub fn browser_status(&self) -> BrowserStatus {
        let custom = self.settings().chrome_path.trim().to_string();
        let browsers: Vec<DetectedBrowser> = detect_browsers()
            .into_iter()
            .map(|b| DetectedBrowser { name: b.name, path: b.path.display().to_string(), version: b.version })
            .collect();
        let custom_missing = !custom.is_empty() && !std::path::Path::new(&custom).is_file();
        let selected = if !custom.is_empty() && !custom_missing { Some(custom) } else { browsers.first().map(|b| b.path.clone()) };
        BrowserStatus { browsers, selected, custom_missing }
    }

    /// Abre (ou reusa) o navegador e devolve a versão. Confirma que ele aceita
    /// ser controlado pelo Snipr.
    pub async fn test_browser(self: &Arc<Self>) -> Result<String> {
        self.active.fetch_add(1, Ordering::SeqCst);
        let r = self.browser().await.map(|b| b.version.clone());
        self.release();
        r
    }

    pub(crate) async fn browser(&self) -> Result<Arc<Browser>> {
        let mut guard = self.browser.lock().await;
        if let Some(b) = guard.as_ref().filter(|b| b.is_alive()) {
            return Ok(b.clone());
        }
        let s = self.settings();
        let b = Browser::launch(LaunchOptions {
            chrome_path: Some(s.chrome_path.trim()).filter(|p| !p.is_empty()).map(PathBuf::from),
            profile_dir: self.paths.profile_dir.clone(),
            mode: s.browser_mode,
            block_media: s.block_media,
            low_memory: s.low_memory,
        })
        .await?;
        *guard = Some(b.clone());
        Ok(b)
    }

    pub async fn close_browser(&self) {
        if let Some(b) = self.browser.lock().await.take() {
            b.close().await;
        }
    }

    pub async fn shutdown(&self) {
        for (_, c) in self.jobs.lock().drain() {
            c.cancel();
        }
        self.gates.cancel_all();
        self.close_browser().await;
    }

    /// Abre o site numa janela visível para o usuário fazer login uma vez.
    /// A sessão fica salva no perfil do Snipr.
    pub async fn open_login(self: &Arc<Self>, site_id: &str) -> Result<()> {
        if self.settings().browser_mode == BrowserMode::Headless {
            bail!("O login não funciona no modo headless. Mude o navegador para Oculto ou Visível em Ajustes.");
        }
        self.close_login(false).await;
        let site = self.sites().into_iter().find(|s| s.id == site_id).ok_or_else(|| anyhow!("loja desconhecida"))?;
        let url = site.login_url.clone().unwrap_or_else(|| site.home());
        self.active.fetch_add(1, Ordering::SeqCst);
        let r: Result<()> = async {
            let browser = self.browser().await?;
            let page = browser.new_page().await?;
            page.set_visible(true).await?;
            page.goto(&url).await?;
            *self.login_page.lock().await = Some((site.id.clone(), page));
            Ok(())
        }
        .await;
        if r.is_err() {
            self.release();
        }
        r
    }

    pub async fn close_login(self: &Arc<Self>, done: bool) {
        if let Some((site, p)) = self.login_page.lock().await.take() {
            p.close().await;
            self.release();
            if done {
                let _ = self.set_session(&site, true);
            }
        }
    }

    pub(crate) fn hold(&self) {
        self.active.fetch_add(1, Ordering::SeqCst);
    }

    pub(crate) fn release(self: &Arc<Self>) {
        if self.active.fetch_sub(1, Ordering::SeqCst) == 1 {
            self.generation.fetch_add(1, Ordering::SeqCst);
            self.schedule_idle_close();
        }
    }

    /// Ocioso: fecha as janelas estacionadas logo, e o Chrome depois de
    /// `idle_close_secs` (0 = mantém o Chrome aberto).
    fn schedule_idle_close(self: &Arc<Self>) {
        const DRAIN_AFTER: u64 = 60;
        let secs = self.settings().idle_close_secs;
        let gen = self.generation.load(Ordering::SeqCst);
        let me = self.clone();
        self.rt.spawn(async move {
            let still_idle = || me.active.load(Ordering::SeqCst) == 0 && me.generation.load(Ordering::SeqCst) == gen;
            let first = if secs == 0 { DRAIN_AFTER } else { secs.min(DRAIN_AFTER) };
            tokio::time::sleep(Duration::from_secs(first)).await;
            if !still_idle() {
                return;
            }
            if secs == 0 || secs > first {
                let browser = me.browser.lock().await.clone();
                if let Some(b) = browser {
                    tracing::info!("navegador ocioso: fechando as janelas estacionadas");
                    b.drain_pool().await;
                }
                if secs == 0 {
                    return;
                }
                tokio::time::sleep(Duration::from_secs(secs - first)).await;
                if !still_idle() {
                    return;
                }
            }
            tracing::info!("navegador ocioso: fechando para liberar memória");
            me.close_browser().await;
        });
    }

    // ---------- buscas ----------

    pub fn resolve_human(&self, agent: &str, ok: bool) -> bool {
        self.gates.resolve(agent, ok)
    }

    pub fn cancel(&self, job: &str) {
        if let Some(c) = self.jobs.lock().get(job) {
            c.cancel();
        }
    }

    pub async fn cancel_all(&self) {
        let tokens: Vec<CancellationToken> = self.jobs.lock().values().cloned().collect();
        for c in &tokens {
            c.cancel();
        }
        self.gates.cancel_all();
        let start = Instant::now();
        while !self.jobs.lock().is_empty() && start.elapsed() < Duration::from_secs(15) {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// Inicia uma busca e devolve o id do job. Os resultados chegam em `sink`.
    pub fn start_search(self: &Arc<Self>, req: SearchRequest, sink: EventSink) -> Result<String> {
        if req.query.trim().len() < 2 {
            bail!("Descreva o produto que você quer buscar.");
        }
        let settings = self.settings();
        self.backend(&settings.ai, self.paths.work_dir.clone(), &settings.ai.model, None)?;
        let all_sites = settings.all_sites();
        let wanted = if req.sites.is_empty() { settings.enabled_sites.clone() } else { req.sites.clone() };
        let sites: Vec<Site> = all_sites.iter().filter(|s| wanted.contains(&s.id)).cloned().collect();
        if sites.is_empty() {
            bail!("Selecione pelo menos uma loja.");
        }

        let job = uuid::Uuid::new_v4().simple().to_string()[..10].to_string();
        let cancel = CancellationToken::new();
        self.jobs.lock().insert(job.clone(), cancel.clone());
        self.hold();

        let me = self.clone();
        let job_id = job.clone();
        self.rt.spawn(async move {
            let started = Instant::now();
            let listings = Arc::new(Mutex::new(Vec::<Listing>::new()));
            me.run_job(&job_id, req, settings, all_sites, sites, &sink, listings.clone(), cancel.clone()).await;
            let total = listings.lock().len();
            let _ = sink.send(SearchEvent::Finished {
                job: job_id.clone(),
                total,
                elapsed_ms: started.elapsed().as_millis(),
                cancelled: cancel.is_cancelled(),
            });
            me.jobs.lock().remove(&job_id);
            me.release();
        });
        Ok(job)
    }
}

pub(crate) fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}
