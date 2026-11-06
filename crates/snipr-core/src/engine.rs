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
use snipr_browser::{Browser, BrowserMode, LaunchOptions, Page};
use snipr_llm::{
    anthropic::{AnthropicConfig, AnthropicSession, DEFAULT_MODEL},
    openai::{OpenAiConfig, OpenAiSession},
    ChatSession,
};
use tokio::sync::{mpsc, Semaphore};
use tokio_util::sync::CancellationToken;

use crate::{
    agent::run_chat_agent,
    claude_code::{self, ClaudeCodeRun},
    config::{Paths, ProviderKind, Settings},
    mcp::McpServer,
    prompts, secrets,
    sites::Site,
    tools::{HumanGates, ToolBox, ToolBoxInit},
    types::{AgentInfo, AgentMode, Listing, SearchEvent, SearchRequest},
};

pub type EventSink = mpsc::UnboundedSender<SearchEvent>;

pub struct Engine {
    paths: Paths,
    settings: RwLock<Settings>,
    browser: tokio::sync::Mutex<Option<Arc<Browser>>>,
    mcp: Arc<McpServer>,
    gates: Arc<HumanGates>,
    jobs: Mutex<HashMap<String, CancellationToken>>,
    active: AtomicUsize,
    generation: AtomicU64,
    login_page: tokio::sync::Mutex<Option<Arc<Page>>>,
    /// Runtime onde o Engine foi criado. Os métodos síncronos (ex.: `start_search`)
    /// podem ser chamados de qualquer thread — inclusive a thread da interface,
    /// que não tem runtime Tokio — e todo trabalho assíncrono vai para cá.
    rt: tokio::runtime::Handle,
}

impl Engine {
    pub async fn new() -> Result<Arc<Self>> {
        let paths = Paths::resolve()?;
        // .env opcional: na pasta atual e na pasta de configuração.
        let _ = dotenvy::dotenv();
        if let Some(dir) = paths.config_file.parent() {
            let _ = dotenvy::from_path(dir.join(".env"));
        }
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
            rt: tokio::runtime::Handle::current(),
        }))
    }

    pub fn settings(&self) -> Settings {
        self.settings.read().clone()
    }

    pub async fn save_settings(&self, mut s: Settings) -> Result<()> {
        s.max_parallel = s.max_parallel.clamp(1, 8);
        s.max_steps = s.max_steps.clamp(5, 200);
        let browser_changed = {
            let old = self.settings.read();
            old.browser_mode != s.browser_mode
                || old.block_media != s.block_media
                || old.low_memory != s.low_memory
                || old.chrome_path != s.chrome_path
        };
        self.paths.save_settings(&s)?;
        *self.settings.write() = s;
        if browser_changed && self.active.load(Ordering::SeqCst) == 0 {
            self.close_browser().await;
        }
        Ok(())
    }

    pub fn sites(&self) -> Vec<Site> {
        self.settings.read().all_sites()
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

    pub fn add_credential(&self, domain: &str, username: &str, password: &str) -> Result<String> {
        let d = secrets::normalize_domain(domain);
        if d.is_empty() || !d.contains('.') {
            bail!("domínio inválido");
        }
        secrets::set(&secrets::site_key(&d, "username"), username)?;
        secrets::set(&secrets::site_key(&d, "password"), password)?;
        let mut s = self.settings();
        if !s.credential_domains.contains(&d) {
            s.credential_domains.push(d.clone());
            self.paths.save_settings(&s)?;
            *self.settings.write() = s;
        }
        Ok(d)
    }

    pub fn remove_credential(&self, domain: &str) -> Result<()> {
        let d = secrets::normalize_domain(domain);
        secrets::delete(&secrets::site_key(&d, "username"))?;
        secrets::delete(&secrets::site_key(&d, "password"))?;
        let mut s = self.settings();
        s.credential_domains.retain(|x| x != &d);
        self.paths.save_settings(&s)?;
        *self.settings.write() = s;
        Ok(())
    }

    // ---------- navegador ----------

    async fn browser(&self) -> Result<Arc<Browser>> {
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
    pub async fn open_login(&self, site_id: &str) -> Result<()> {
        if self.settings().browser_mode == BrowserMode::Headless {
            bail!("Para conectar contas, mude o navegador para 'oculto' ou 'visível' nos ajustes.");
        }
        self.close_login().await;
        let site = self.sites().into_iter().find(|s| s.id == site_id).ok_or_else(|| anyhow!("site desconhecido"))?;
        let url = site.login_url.clone().unwrap_or_else(|| format!("https://www.{}/", site.domain));
        self.active.fetch_add(1, Ordering::SeqCst);
        let r: Result<()> = async {
            let browser = self.browser().await?;
            let page = browser.new_page().await?;
            page.set_visible(true).await?;
            page.goto(&url).await?;
            *self.login_page.lock().await = Some(page);
            Ok(())
        }
        .await;
        if r.is_err() {
            self.release();
        }
        r
    }

    pub async fn close_login(&self) {
        if let Some(p) = self.login_page.lock().await.take() {
            p.close().await;
            self.release();
        }
    }

    fn release(&self) {
        if self.active.fetch_sub(1, Ordering::SeqCst) == 1 {
            self.generation.fetch_add(1, Ordering::SeqCst);
        }
    }

    /// Fecha o Chrome depois de um tempo ocioso para devolver a RAM.
    fn schedule_idle_close(self: &Arc<Self>) {
        let secs = self.settings().idle_close_secs;
        if secs == 0 {
            return;
        }
        let gen = self.generation.load(Ordering::SeqCst);
        let me = self.clone();
        self.rt.spawn(async move {
            tokio::time::sleep(Duration::from_secs(secs)).await;
            if me.active.load(Ordering::SeqCst) == 0 && me.generation.load(Ordering::SeqCst) == gen {
                tracing::info!("navegador ocioso: fechando para liberar memória");
                me.close_browser().await;
            }
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

    /// Inicia uma busca e devolve o id do job. Os resultados chegam em `sink`.
    pub fn start_search(self: &Arc<Self>, req: SearchRequest, sink: EventSink) -> Result<String> {
        if req.query.trim().len() < 2 {
            bail!("Descreva o produto que você quer procurar.");
        }
        let settings = self.settings();
        self.check_provider(&settings)?;
        let all_sites = settings.all_sites();
        let wanted = if req.sites.is_empty() { settings.enabled_sites.clone() } else { req.sites.clone() };
        let sites: Vec<Site> = all_sites.iter().filter(|s| wanted.contains(&s.id)).cloned().collect();
        if sites.is_empty() {
            bail!("Selecione pelo menos um site.");
        }

        let job = uuid::Uuid::new_v4().simple().to_string()[..10].to_string();
        let cancel = CancellationToken::new();
        self.jobs.lock().insert(job.clone(), cancel.clone());
        self.active.fetch_add(1, Ordering::SeqCst);

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
            me.schedule_idle_close();
        });
        Ok(job)
    }

    fn check_provider(&self, s: &Settings) -> Result<()> {
        match s.provider {
            ProviderKind::ClaudeCode => {
                claude_code::find_claude(&s.claude_path).ok_or_else(|| {
                    anyhow!("Claude Code não encontrado. Instale-o ou informe o caminho do executável nos ajustes.")
                })?;
            }
            ProviderKind::Anthropic => {
                if !secrets::has("anthropic_api_key") {
                    bail!("Cadastre sua API key da Anthropic nos ajustes.");
                }
            }
            ProviderKind::OpenaiCompat => {
                if s.model.trim().is_empty() {
                    bail!("Informe o modelo (ex.: o nome do modelo do provedor) nos ajustes.");
                }
            }
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn run_job(
        self: &Arc<Self>,
        job: &str,
        req: SearchRequest,
        settings: Settings,
        all_sites: Vec<Site>,
        sites: Vec<Site>,
        sink: &EventSink,
        listings: Arc<Mutex<Vec<Listing>>>,
        cancel: CancellationToken,
    ) {
        let groups: Vec<(String, Vec<Site>)> = match req.mode {
            AgentMode::Single => vec![("Agente".into(), sites)],
            AgentMode::Multi => sites.into_iter().map(|s| (s.name.clone(), vec![s])).collect(),
        };
        let agents: Vec<AgentInfo> = groups
            .iter()
            .enumerate()
            .map(|(i, (label, s))| AgentInfo {
                id: format!("{job}-{i}"),
                label: label.clone(),
                sites: s.iter().map(|x| x.id.clone()).collect(),
            })
            .collect();
        let _ = sink.send(SearchEvent::Started { job: job.into(), agents: agents.clone() });

        let browser = match self.browser().await {
            Ok(b) => b,
            Err(e) => {
                for a in &agents {
                    let _ = sink.send(SearchEvent::AgentFinished {
                        job: job.into(),
                        agent: a.id.clone(),
                        ok: false,
                        summary: format!("{e:#}"),
                    });
                }
                return;
            }
        };

        let parallel = if req.mode == AgentMode::Multi { settings.max_parallel } else { 1 };
        let sem = Arc::new(Semaphore::new(parallel));
        let mut set = tokio::task::JoinSet::new();
        for (info, (_, group)) in agents.into_iter().zip(groups) {
            let me = self.clone();
            let sem = sem.clone();
            let browser = browser.clone();
            let sink = sink.clone();
            let req = req.clone();
            let settings = settings.clone();
            let all_sites = all_sites.clone();
            let listings = listings.clone();
            let cancel = cancel.clone();
            let job = job.to_string();
            set.spawn(async move {
                let Ok(_permit) = sem.acquire_owned().await else { return };
                if cancel.is_cancelled() {
                    return;
                }
                let r = me
                    .run_agent(&job, &info, &req, &settings, all_sites, group, browser, &sink, listings, cancel)
                    .await;
                let (ok, summary) = match r {
                    Ok(s) => (true, s),
                    Err(e) => (false, format!("{e:#}")),
                };
                let _ = sink.send(SearchEvent::AgentFinished { job, agent: info.id.clone(), ok, summary });
            });
        }
        while set.join_next().await.is_some() {}
    }

    #[allow(clippy::too_many_arguments)]
    async fn run_agent(
        &self,
        job: &str,
        info: &AgentInfo,
        req: &SearchRequest,
        settings: &Settings,
        all_sites: Vec<Site>,
        sites: Vec<Site>,
        browser: Arc<Browser>,
        sink: &EventSink,
        listings: Arc<Mutex<Vec<Listing>>>,
        cancel: CancellationToken,
    ) -> Result<String> {
        let page = browser.new_page().await?;
        let task = prompts::task(req, &sites, &settings.credential_domains);
        let tools = Arc::new(ToolBox::new(ToolBoxInit {
            job: job.to_string(),
            agent: info.id.clone(),
            page: page.clone(),
            sites,
            all_sites,
            filter: req.condition,
            max_price: req.max_price,
            credential_domains: settings.credential_domains.clone(),
            events: sink.clone(),
            gates: self.gates.clone(),
            listings,
        }));
        tools.log(format!(
            "iniciando ({})",
            tools.assigned_sites().iter().map(|s| s.name.as_str()).collect::<Vec<_>>().join(", ")
        ));

        let effort = Some(settings.effort.trim().to_string()).filter(|e| !e.is_empty());
        let result = match settings.provider {
            ProviderKind::ClaudeCode => {
                self.mcp.register(&info.id, tools.clone());
                let run = ClaudeCodeRun {
                    binary: claude_code::find_claude(&settings.claude_path)
                        .ok_or_else(|| anyhow!("Claude Code não encontrado"))?,
                    model: Some(settings.model.trim().to_string()).filter(|m| !m.is_empty()),
                    effort,
                    oauth_token: secrets::reveal("claude_code_oauth_token"),
                    work_dir: self.paths.work_dir.clone(),
                    mcp_url: self.mcp.url_for(&info.id),
                    mcp_token: self.mcp.token().to_string(),
                };
                let r = claude_code::run(run, prompts::SYSTEM, task, tools.clone(), cancel).await;
                self.mcp.unregister(&info.id);
                r
            }
            ProviderKind::Anthropic | ProviderKind::OpenaiCompat => {
                let session: Box<dyn ChatSession> = if settings.provider == ProviderKind::Anthropic {
                    let model = Some(settings.model.trim()).filter(|m| !m.is_empty()).unwrap_or(DEFAULT_MODEL);
                    Box::new(AnthropicSession::new(
                        AnthropicConfig {
                            api_key: secrets::reveal("anthropic_api_key")
                                .ok_or_else(|| anyhow!("API key da Anthropic ausente"))?,
                            model: model.to_string(),
                            effort,
                            base_url: "https://api.anthropic.com".into(),
                        },
                        prompts::SYSTEM.to_string(),
                        &ToolBox::specs(),
                    ))
                } else {
                    Box::new(OpenAiSession::new(
                        OpenAiConfig {
                            base_url: settings.openai_base_url.clone(),
                            api_key: secrets::reveal("openai_api_key"),
                            model: settings.model.trim().to_string(),
                        },
                        prompts::SYSTEM.to_string(),
                        &ToolBox::specs(),
                    ))
                };
                run_chat_agent(session, task, tools.clone(), settings.max_steps, cancel).await
            }
        };
        page.close().await;
        result
    }
}
