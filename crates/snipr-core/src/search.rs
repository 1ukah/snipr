//! Uma busca, do plano aos anúncios.
//!
//! 1. Plano (modelo rápido).
//! 2. Cada loja é lida na etapa dela (A/B/C por HTTP, D no Chrome), sem IA.
//!    Se a etapa falhar, sobe para a próxima e grava a nova etapa.
//! 3. Filtros locais (preço, palavras do plano, repetidos).
//! 4. Uma chamada curta à IA por lote de lojas devolve só os ids que atendem
//!    ao plano. Anúncios duvidosos têm a página lida e passam por uma
//!    segunda chamada.
//! 5. O agent (IA com ferramentas) só entra em lojas que o Snipr não
//!    consegue ler sozinho.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::Arc,
    time::Duration,
};

use anyhow::{anyhow, bail, Result};
use futures_util::StreamExt;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use snipr_browser::Card;
use tokio::sync::{mpsc, oneshot, Semaphore};
use tokio_util::sync::CancellationToken;

use crate::{
    agent::run_chat_agent,
    cli::{self, Mcp},
    config::Settings,
    engine::{unix_now, Backend, Engine, EventSink, Tier},
    fetch, offers, parse, prompts,
    sites::{Api, Site, Stage},
    tools::{listing_from_card, Sink, SessionHook, ToolBox, ToolBoxInit},
    types::{AgentInfo, Condition, Listing, SearchEvent, SearchPlan, SearchRequest},
};

/// Uma etapa mais leve volta a ser testada depois deste tempo. Um bloqueio
/// passageiro não deixa a loja numa etapa mais cara por muito tempo.
const RETRY_LIGHTER_AFTER: u64 = 24 * 3600;
/// Cards por loja que vão para o filtro da IA.
const MAX_CANDIDATES: usize = 40;
/// Cards lidos por página de resultados (a OLX mostra 50). Ler é barato; o
/// limite de custo é `MAX_CANDIDATES`.
const MAX_PER_PAGE: usize = 60;
/// Anúncios abertos para conferir detalhes, por lote.
const MAX_CHECKS: usize = 6;
/// O mesmo, na busca profunda (plano com termos amplos): anúncios genéricos
/// só mostram as especificações na descrição.
const MAX_CHECKS_DEEP: usize = 24;
/// Cards de título genérico por loja que vão para o filtro na busca profunda.
const MAX_GENERIC: usize = 25;
/// Páginas de anúncio por chamada à IA na conferência de detalhes.
const PAGES_PER_CALL: usize = 6;
/// Anúncios por loja cuja página o Snipr lê (sem IA) para achar o preço no
/// PIX, as parcelas e os cupons. Os mais baratos primeiro.
const MAX_ENRICH: usize = 10;

pub(crate) struct Ctx {
    job: String,
    req: SearchRequest,
    plan: SearchPlan,
    settings: Settings,
    all_sites: Vec<Site>,
    sink: Arc<Sink>,
    events: EventSink,
    cancel: CancellationToken,
    /// Abas do Chrome em uso ao mesmo tempo.
    chrome: Arc<Semaphore>,
    /// Anúncios cuja página já foi lida na conferência de detalhes.
    read_pages: Mutex<HashSet<String>>,
    memo: Option<Arc<Mutex<Memo>>>,
}

/// Vereditos da IA de execuções anteriores de uma busca automática. Um
/// anúncio já julgado não volta para a IA: só os novos custam tokens.
#[derive(Default, Serialize, Deserialize)]
pub(crate) struct Memo {
    kept: HashMap<String, (Condition, usize)>,
    dropped: HashSet<String>,
}

impl Memo {
    const MAX: usize = 4000;

    /// `Some(None)`: recusado antes. `Some(Some(..))`: aceito, com condição e preço.
    fn verdict(&self, card: &Card) -> Option<Option<(Condition, usize)>> {
        let k = crate::tools::dedupe_key(&card.url, &card.title);
        if self.dropped.contains(&k) {
            return Some(None);
        }
        self.kept.get(&k).map(|v| Some(*v))
    }

    fn remember(&mut self, card: &Card, hit: Option<(Condition, usize)>) {
        if self.kept.len() + self.dropped.len() >= Self::MAX {
            self.kept.clear();
            self.dropped.clear();
        }
        let k = crate::tools::dedupe_key(&card.url, &card.title);
        match hit {
            Some(v) => {
                self.dropped.remove(&k);
                self.kept.insert(k, v);
            }
            None => {
                self.kept.remove(&k);
                self.dropped.insert(k);
            }
        }
    }
}

enum Got {
    Cards(Vec<Card>),
    /// A loja respondeu que não há resultados.
    NoResults,
    /// A página abriu, mas o Snipr não achou cards nela.
    Unreadable,
}

enum Collected {
    Cards(Vec<Card>, Stage),
    NeedsAgent,
}

struct Batch {
    site: Site,
    agent: String,
    cards: Vec<Card>,
    /// Link usado quando o card não expõe o link do anúncio.
    fallback_url: String,
    /// Anúncios aceitos da loja.
    done: oneshot::Sender<Result<Vec<Listing>, String>>,
}

impl Engine {
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn run_job(
        self: &Arc<Self>,
        job: &str,
        mut req: SearchRequest,
        settings: Settings,
        all_sites: Vec<Site>,
        sites: Vec<Site>,
        events: &EventSink,
        listings: Arc<Mutex<Vec<Listing>>>,
        cancel: CancellationToken,
        memo: Option<Arc<Mutex<Memo>>>,
    ) -> Option<SearchPlan> {
        let agents: Vec<AgentInfo> = sites
            .iter()
            .enumerate()
            .map(|(i, s)| AgentInfo { id: format!("{job}-{i}"), label: s.name.clone(), sites: vec![s.id.clone()] })
            .collect();
        let _ = events.send(SearchEvent::Started { job: job.into(), agents: agents.clone() });

        // O Chrome só abre se alguma loja precisar dele; abre junto com o plano.
        let needs_chrome = sites.iter().any(|s| !matches!(s.stage, Some(Stage::A | Stage::B)) || needs_agent(s));
        let given = req.plan.take();
        let (plan, usage) = if needs_chrome {
            let (p, _) = tokio::join!(self.plan_or(&settings, &req.query, given, &cancel), async {
                if let Err(e) = self.browser().await {
                    tracing::warn!("navegador indisponível: {e:#}");
                }
            });
            p
        } else {
            self.plan_or(&settings, &req.query, given, &cancel).await
        };
        if !usage.is_empty() {
            let _ = events.send(SearchEvent::Usage { job: job.into(), agent: None, usage });
        }
        if cancel.is_cancelled() {
            return None;
        }
        let _ = events.send(SearchEvent::Planned { job: job.into(), plan: plan.clone() });
        if req.max_price.is_none() {
            req.max_price = plan.max_price;
        }
        if let Some(c) = plan.condition {
            req.condition = c;
        }

        let sink = Arc::new(Sink {
            job: job.to_string(),
            events: events.clone(),
            listings,
            filter: req.condition,
            max_price: req.max_price,
        });
        let ctx = Arc::new(Ctx {
            job: job.to_string(),
            req,
            plan,
            chrome: Arc::new(Semaphore::new(settings.max_parallel.max(1))),
            settings,
            all_sites,
            sink,
            events: events.clone(),
            cancel: cancel.clone(),
            read_pages: Default::default(),
            memo,
        });

        let (tx, rx) = mpsc::unbounded_channel::<Batch>();
        let judge = tokio::spawn({
            let me = self.clone();
            let ctx = ctx.clone();
            async move { me.judge_loop(ctx, rx).await }
        });

        let mut set = tokio::task::JoinSet::new();
        for (info, site) in agents.into_iter().zip(sites) {
            let me = self.clone();
            let ctx = ctx.clone();
            let tx = tx.clone();
            set.spawn(async move {
                let work = me.run_site(&ctx, &info, &site, tx);
                let r = tokio::select! {
                    r = work => r,
                    _ = ctx.cancel.cancelled() => Err(anyhow!("cancelado")),
                };
                let (ok, summary) = match r {
                    Ok(s) => (true, s),
                    Err(e) => (false, format!("{e:#}")),
                };
                let _ = ctx.events.send(SearchEvent::AgentFinished { job: ctx.job.clone(), agent: info.id.clone(), ok, summary });
            });
        }
        drop(tx);
        while set.join_next().await.is_some() {}
        let _ = judge.await;
        Some(ctx.plan.clone())
    }

    async fn plan_or(&self, settings: &Settings, query: &str, given: Option<SearchPlan>, cancel: &CancellationToken) -> (SearchPlan, crate::types::Usage) {
        match given {
            Some(p) => (p, Default::default()),
            None => self.plan_usage(&settings.ai, query, cancel.clone()).await,
        }
    }

    async fn run_site(self: &Arc<Self>, ctx: &Arc<Ctx>, info: &AgentInfo, site: &Site, judge: mpsc::UnboundedSender<Batch>) -> Result<String> {
        let agent = info.id.as_str();
        match self.collect(ctx, site, agent).await? {
            Collected::NeedsAgent => {
                ctx.sink.log(agent, "sem leitura automática: usando o agent");
                self.run_agent(ctx, agent, site).await
            }
            Collected::Cards(cards, stage) if cards.is_empty() => Ok(format!("nenhum anúncio encontrado · etapa {}", stage.label())),
            Collected::Cards(cards, stage) => {
                ctx.sink.log(agent, format!("{} anúncio(s) lido(s); filtrando", cards.len()));
                let (done, result) = oneshot::channel();
                let fallback_url = site.build_search_url(terms(ctx, site).first().map(String::as_str).unwrap_or(&ctx.req.query));
                judge
                    .send(Batch { site: site.clone(), agent: agent.to_string(), cards, fallback_url, done })
                    .map_err(|_| anyhow!("filtro indisponível"))?;
                let kept = result.await.map_err(|_| anyhow!("filtro interrompido"))?.map_err(|e| anyhow!(e))?;
                let n = kept.len();
                if ctx.settings.offer_details {
                    self.enrich(ctx, site, agent, stage, kept).await;
                }
                Ok(format!("{n} anúncio(s) · etapa {}", stage.label()))
            }
        }
    }

    // ---------- leitura das lojas ----------

    async fn collect(self: &Arc<Self>, ctx: &Arc<Ctx>, site: &Site, agent: &str) -> Result<Collected> {
        if needs_agent(site) {
            return Ok(Collected::NeedsAgent);
        }
        let configured = site.stage.unwrap_or(Stage::D);
        let checked = ctx.settings.site_stages.get(&site.id).map(|i| i.checked_at).unwrap_or(0);
        let stale = unix_now().saturating_sub(checked) > RETRY_LIGHTER_AFTER;
        // Etapa mais leve primeiro, de tempos em tempos; depois a gravada e as mais pesadas.
        let mut order = Vec::new();
        if let Some(p) = configured.prev().filter(|_| stale) {
            order.push(p);
        }
        let mut s = Some(configured);
        while let Some(x) = s {
            order.push(x);
            s = x.next();
        }

        let mut last = anyhow!("nenhuma etapa disponível");
        for stage in order {
            if ctx.cancel.is_cancelled() {
                bail!("cancelado");
            }
            match self.fetch_stage(ctx, site, stage, agent).await {
                Ok(Got::Cards(cards)) => {
                    if stage != configured || stale {
                        self.record_stage(site, stage);
                    }
                    return Ok(Collected::Cards(cards, stage));
                }
                Ok(Got::NoResults) => {
                    if stage != configured || stale {
                        self.record_stage(site, stage);
                    }
                    return Ok(Collected::Cards(Vec::new(), stage));
                }
                Ok(Got::Unreadable) if stage == Stage::D => return Ok(Collected::NeedsAgent),
                Ok(Got::Unreadable) => {
                    ctx.sink.log(agent, format!("etapa {}: nenhum card lido; tentando a próxima", stage.label()));
                    last = anyhow!("nenhum card lido");
                }
                Err(e) => {
                    ctx.sink.log(agent, format!("etapa {}: {e:#}", stage.label()));
                    last = e;
                }
            }
        }
        Err(last)
    }

    async fn fetch_stage(self: &Arc<Self>, ctx: &Arc<Ctx>, site: &Site, stage: Stage, agent: &str) -> Result<Got> {
        if stage == Stage::D {
            return self.chrome_cards(ctx, site, agent).await;
        }
        let mut cookie = None;
        if stage == Stage::C {
            cookie = Some(self.cookie_for(ctx, site).await?);
        }
        let (mut all, mut no_results) = (Vec::<Card>::new(), false);
        let mut queue = Queue::new(terms(ctx, site), 4);
        let mut first = true;
        while let Some((term, retry)) = queue.next() {
            if !first {
                // Uma pessoa não dispara buscas em sequência instantânea.
                tokio::time::sleep(Duration::from_millis(snipr_rand(400, 900))).await;
            }
            first = false;
            ctx.sink.log(agent, format!("buscando \"{term}\" (etapa {}){}", stage.label(), if retry { ", mais curto" } else { "" }));
            let url = match site.api {
                Some(Api::Vtex) => vtex_url(site, &term),
                None => site.build_search_url(&term),
            };
            let mut page = self.fetcher.get(stage, &url, cookie.as_deref()).await?;
            if let Some(why) = fetch::blocked(&page) {
                if stage != Stage::C {
                    bail!("{why}");
                }
                // Cookies vencidos: o Chrome passa pela proteção de novo, uma vez.
                self.fetcher.forget_cookie(&site.id);
                cookie = Some(self.cookie_for(ctx, site).await?);
                page = self.fetcher.get(stage, &url, cookie.as_deref()).await?;
                if let Some(why) = fetch::blocked(&page) {
                    bail!("{why}");
                }
            }
            let cards = match site.api {
                Some(Api::Vtex) => parse::cards_from_vtex(&page.body, MAX_PER_PAGE),
                None => parse::cards_from_html(&page.body, &page.url, MAX_PER_PAGE, Some(site.json_url.as_str()).filter(|j| !j.is_empty())),
            };
            queue.after(&term, retry, &ctx.plan.product, &cards);
            if cards.is_empty() {
                // Um termo sem cards (ex.: outra marca na loja de uma marca) não
                // invalida os outros: a etapa só falha se nenhum termo render.
                no_results |= fetch::says_no_results(&page.body);
                continue;
            }
            merge(&mut all, cards);
        }
        Ok(if !all.is_empty() {
            Got::Cards(all)
        } else if no_results {
            Got::NoResults
        } else {
            Got::Unreadable
        })
    }

    /// Cookies da loja obtidos pelo Chrome (etapa C). Ficam na memória por alguns minutos.
    async fn cookie_for(self: &Arc<Self>, ctx: &Arc<Ctx>, site: &Site) -> Result<String> {
        if let Some(c) = self.fetcher.cookie(&site.id) {
            return Ok(c);
        }
        let _slot = ctx.chrome.acquire().await?;
        let browser = self.browser().await?;
        let page = browser.lease().await?;
        let search = site.build_search_url(terms(ctx, site).first().map(String::as_str).unwrap_or(&ctx.req.query));
        page.goto(&site.home()).await?;
        let cookie = page.cookie_header(&[&site.home(), &search]).await?;
        if cookie.is_empty() {
            bail!("o Chrome não recebeu cookies da loja");
        }
        self.fetcher.set_cookie(&site.id, cookie.clone());
        Ok(cookie)
    }

    fn toolbox(self: &Arc<Self>, ctx: &Arc<Ctx>, agent: &str, site: &Site, page: Arc<snipr_browser::Page>) -> Arc<ToolBox> {
        let me = self.clone();
        let on_session: SessionHook = Arc::new(move |site: &str, ok: bool| {
            if let Err(e) = me.set_session(site, ok) {
                tracing::warn!("não foi possível salvar a sessão de {site}: {e:#}");
            }
        });
        Arc::new(ToolBox::new(ToolBoxInit {
            agent: agent.to_string(),
            page,
            sites: vec![site.clone()],
            all_sites: ctx.all_sites.clone(),
            sessions: ctx.settings.sessions.clone(),
            on_session,
            gates: self.gates.clone(),
            sink: ctx.sink.clone(),
        }))
    }

    /// Etapa D: o Chrome abre a busca e o Snipr lê os cards, sem IA.
    async fn chrome_cards(self: &Arc<Self>, ctx: &Arc<Ctx>, site: &Site, agent: &str) -> Result<Got> {
        let _slot = ctx.chrome.acquire().await?;
        let browser = self.browser().await?;
        let lease = browser.lease().await?;
        let tools = self.toolbox(ctx, agent, site, lease.page().clone());
        let (mut all, mut no_results) = (Vec::<Card>::new(), false);
        // Um termo por alternativa do plano (no máximo 4): cortar termos aqui
        // deixa alternativas inteiras sem busca. Cada página no Chrome custa
        // CPU: no máximo uma busca mais curta.
        let mut queue = Queue::new(terms(ctx, site), 1);
        while let Some((term, retry)) = queue.next() {
            ctx.sink.log(agent, format!("buscando \"{term}\" (etapa D){}", if retry { ", mais curto" } else { "" }));
            let msg = tools.navigate(&site.build_search_url(&term)).await?;
            if !msg.starts_with("Página aberta") {
                bail!("{}", msg.lines().next().unwrap_or_default());
            }
            let mut cards = lease.extract_cards(MAX_PER_PAGE).await?;
            if cards.len() < 8 {
                // Rolagem infinita (ex.: Marketplace) carrega mais cards.
                lease.scroll(true).await?;
                let more = lease.extract_cards(MAX_PER_PAGE).await?;
                if more.len() > cards.len() {
                    cards = more;
                }
            }
            queue.after(&term, retry, &ctx.plan.product, &cards);
            if cards.is_empty() {
                no_results |= fetch::says_no_results(&lease.read_text(4000).await.unwrap_or_default());
                continue;
            }
            merge(&mut all, cards);
        }
        Ok(if !all.is_empty() {
            Got::Cards(all)
        } else if no_results {
            Got::NoResults
        } else {
            Got::Unreadable
        })
    }

    // ---------- filtro com IA ----------

    async fn judge_loop(self: Arc<Self>, ctx: Arc<Ctx>, mut rx: mpsc::UnboundedReceiver<Batch>) {
        while let Some(first) = rx.recv().await {
            // Lojas que terminam quase juntas vão no mesmo lote: menos chamadas à IA.
            let mut batches = vec![first];
            let deadline = tokio::time::Instant::now() + Duration::from_millis(1500);
            while let Ok(Some(b)) = tokio::time::timeout_at(deadline, rx.recv()).await {
                batches.push(b);
            }
            self.judge(&ctx, batches).await;
        }
    }

    async fn judge(self: &Arc<Self>, ctx: &Arc<Ctx>, batches: Vec<Batch>) {
        let words: Vec<String> = parse::tokens(&format!(
            "{} {} {}",
            ctx.plan.product,
            ctx.plan.alternatives.join(" "),
            ctx.plan.search_terms.join(" ")
        ));
        let groups = parse::plan_groups(&ctx.plan.product, &ctx.plan.alternatives, &ctx.plan.search_terms);
        let strict = !ctx.plan.alternatives.is_empty();
        let deep = !ctx.plan.broad_terms.is_empty();
        let max = ctx.req.max_price;
        let mut cand: Vec<(usize, Card)> = Vec::new();
        let mut rows = Vec::new();
        // Ids enviados à IA e ids aceitos de execuções anteriores (com condição e preço).
        let (mut asked, mut remembered) = (Vec::<usize>::new(), Vec::<(usize, Condition, usize)>::new());
        for (bi, b) in batches.iter().enumerate() {
            let usable: Vec<&Card> = b
                .cards
                .iter()
                .filter(|card| {
                    let title = parse::fold(&card.title);
                    let min = parse::sorted_prices(card).first().copied();
                    !title.starts_with("compro")
                        && !title.starts_with("procuro")
                        && min.is_some_and(|min| min >= 5.0 && !max.is_some_and(|m| min > m))
                })
                .collect();
            let mut short = parse::shortlist(&usable, &groups, &ctx.plan.product, strict, &words, MAX_CANDIDATES);
            if deep {
                let extra = parse::generic(&usable, &short, &ctx.plan.product, &ctx.plan.broad_terms, MAX_GENERIC);
                short.extend(extra);
            }
            for card in short {
                // Busca automática: anúncios já julgados numa execução anterior não voltam para a IA.
                match ctx.memo.as_ref().and_then(|m| m.lock().verdict(card)) {
                    Some(None) => continue,
                    Some(Some((cond, pidx))) => remembered.push((cand.len() + 1, cond, pidx)),
                    None => {
                        rows.push(prompts::judge_row(cand.len() + 1, &b.site.name, card));
                        asked.push(cand.len() + 1);
                    }
                }
                cand.push((bi, card.clone()));
            }
        }
        let max_checks = if deep { MAX_CHECKS_DEEP } else { MAX_CHECKS };
        let mut kept: Vec<Vec<Listing>> = vec![Vec::new(); batches.len()];
        for (id, cond, pidx) in &remembered {
            self.keep(ctx, &batches, &cand, *id, *cond, *pidx, None, &mut kept);
        }
        if rows.is_empty() {
            for (b, k) in batches.into_iter().zip(kept) {
                let _ = b.done.send(Ok(k));
            }
            return;
        }
        let names: Vec<&Site> = batches.iter().map(|b| &b.site).collect();
        let prompt = format!(
            "{}\n{}\nLimite de check: {max_checks} anúncios.\n\nAnúncios:\n{}",
            prompts::plan_text(&ctx.req, &ctx.plan),
            prompts::stores_legend(&names),
            rows.join("\n")
        );
        for b in &batches {
            ctx.sink.log(&b.agent, "IA filtrando os anúncios");
        }
        let verdict = match self.verdict(ctx, prompts::JUDGE, prompt, "judge").await {
            Ok(v) => v,
            Err(e) => {
                let msg = format!("{e:#}");
                for b in batches {
                    let _ = b.done.send(Err(msg.clone()));
                }
                return;
            }
        };

        let mut accepted: Vec<(usize, Condition, usize)> = Vec::new();
        for (id, cond, pidx) in &verdict.keep {
            self.keep(ctx, &batches, &cand, *id, *cond, *pidx, None, &mut kept);
            accepted.push((*id, *cond, *pidx));
        }
        let checks: Vec<usize> = verdict.check.iter().copied().filter(|id| (1..=cand.len()).contains(id)).take(max_checks).collect();
        let mut unsure: Vec<usize> = Vec::new();
        if !checks.is_empty() {
            for b in &batches {
                ctx.sink.log(&b.agent, format!("lendo a descrição de {} anúncio(s)", checks.len()));
            }
            match self.check_details(ctx, &batches, &cand, &checks).await {
                Ok((v, pages)) => {
                    for (id, cond, pidx) in &v.keep {
                        if checks.contains(id) {
                            self.keep(ctx, &batches, &cand, *id, *cond, *pidx, pages.get(id).map(String::as_str), &mut kept);
                            accepted.push((*id, *cond, *pidx));
                        }
                    }
                    // Página que não abriu: sem veredito, a próxima execução pergunta de novo.
                    unsure.extend(checks.iter().filter(|id| !pages.contains_key(id)));
                }
                Err(e) => {
                    tracing::warn!("conferência de detalhes falhou: {e:#}");
                    unsure.extend(&checks);
                }
            }
        }
        if let Some(m) = &ctx.memo {
            let mut m = m.lock();
            for id in asked.iter().filter(|id| !unsure.contains(id)) {
                let card = &cand[id - 1].1;
                let hit = accepted.iter().find(|(a, _, _)| a == id).map(|(_, c, p)| (*c, *p));
                m.remember(card, hit);
            }
        }
        for (b, k) in batches.into_iter().zip(kept) {
            let _ = b.done.send(Ok(k));
        }
    }

    /// Registra o anúncio `id`. `page`: texto da página do anúncio, se já foi lida.
    #[allow(clippy::too_many_arguments)]
    fn keep(&self, ctx: &Ctx, batches: &[Batch], cand: &[(usize, Card)], id: usize, cond: Condition, pidx: usize, page: Option<&str>, kept: &mut [Vec<Listing>]) {
        let Some((bi, card)) = id.checked_sub(1).and_then(|i| cand.get(i)) else { return };
        let b = &batches[*bi];
        let prices = parse::sorted_prices(card);
        let Some(price) = prices.get(pidx).or(prices.first()).copied() else { return };
        let mut listing = listing_from_card(&b.site, card, price, cond, &b.fallback_url);
        if let Some(text) = page {
            offers::apply(&mut listing, &offers::read(text, true));
            ctx.read_pages.lock().insert(listing.url.clone());
        }
        if let Ok(true) = ctx.sink.accept(&b.agent, listing.clone()) {
            kept[*bi].push(listing);
        }
    }

    /// Lê por HTTP a página dos anúncios aceitos (sem IA) e completa o preço
    /// no PIX, as parcelas e os cupons.
    async fn enrich(&self, ctx: &Ctx, site: &Site, agent: &str, stage: Stage, mut kept: Vec<Listing>) {
        {
            let read = ctx.read_pages.lock();
            kept.retain(|l| !read.contains(&l.url) && l.url.starts_with("http"));
        }
        kept.sort_by(|a, b| a.price.total_cmp(&b.price));
        kept.truncate(MAX_ENRICH);
        if kept.is_empty() || ctx.cancel.is_cancelled() {
            return;
        }
        ctx.sink.log(agent, format!("lendo parcelas e cupons de {} anúncio(s)", kept.len()));
        // Etapa D: abrir cada anúncio no Chrome custa caro, mas a página do
        // anúncio costuma abrir por HTTP mesmo quando a busca não abre.
        let cookie = if matches!(stage, Stage::C | Stage::D) { self.fetcher.cookie(&site.id) } else { None };
        let stage = match stage {
            Stage::D if cookie.is_some() => Stage::C,
            Stage::D => Stage::B,
            s => s,
        };
        let fails = std::sync::atomic::AtomicUsize::new(0);
        let work = futures_util::stream::iter(kept).for_each_concurrent(3, |mut l| {
            let (cookie, fails) = (cookie.as_deref(), &fails);
            async move {
                // Dois bloqueios seguidos: a loja não quer mais pedidos agora.
                if fails.load(std::sync::atomic::Ordering::Relaxed) >= 2 || ctx.cancel.is_cancelled() {
                    return;
                }
                let page = match self.fetcher.get(stage, &l.url, cookie).await {
                    Ok(p) if fetch::blocked(&p).is_none() => p,
                    _ => {
                        fails.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        return;
                    }
                };
                let text = parse::text_from_html(&page.body, 12_000);
                if offers::apply(&mut l, &offers::read(&text, true)) {
                    ctx.sink.update(agent, l);
                }
            }
        });
        tokio::select! {
            _ = work => {}
            _ = ctx.cancel.cancelled() => {}
        }
    }

    /// Pergunta ao modelo rápido; se a resposta não vier no formato, repete com o principal.
    async fn verdict(&self, ctx: &Ctx, system: &str, prompt: String, dir: &str) -> Result<prompts::Verdict> {
        let ai = &ctx.settings.ai;
        let mut last = anyhow!("resposta da IA fora do formato");
        for tier in [Tier::Fast, Tier::Main] {
            match self.ask(ai, tier, system, prompt.clone(), dir, ctx.cancel.clone()).await {
                Ok((text, usage)) => {
                    ctx.sink.usage(None, usage);
                    if let Some(v) = prompts::parse_verdict(&text) {
                        return Ok(v);
                    }
                    tracing::warn!(?tier, "resposta do filtro fora do formato");
                }
                Err(e) => {
                    if ctx.cancel.is_cancelled() {
                        return Err(e);
                    }
                    tracing::warn!(?tier, "filtro falhou: {e:#}");
                    last = e;
                }
            }
        }
        Err(last)
    }

    /// Abre os anúncios duvidosos (com seções recolhidas abertas) e pergunta de novo.
    async fn check_details(
        self: &Arc<Self>,
        ctx: &Arc<Ctx>,
        batches: &[Batch],
        cand: &[(usize, Card)],
        ids: &[usize],
    ) -> Result<(prompts::Verdict, HashMap<usize, String>)> {
        let reads = ids.iter().map(|id| async move {
            let (bi, card) = &cand[id - 1];
            let b = &batches[*bi];
            if card.url.is_empty() {
                return None;
            }
            let text = self.page_text(ctx, &b.site, &b.agent, &card.url).await.map_err(|e| tracing::debug!("detalhes de {}: {e:#}", card.url)).ok()?;
            Some((*id, format!("### {id} | {} | {}\n{}\n", b.site.name, card.title, text)))
        });
        let pages: HashMap<usize, String> = futures_util::future::join_all(reads).await.into_iter().flatten().collect();
        if pages.is_empty() {
            bail!("nenhuma página de anúncio pôde ser lida");
        }
        let texts: Vec<&str> = pages.values().map(String::as_str).collect();
        // Poucas páginas por chamada: o prompt fica curto e as chamadas correm juntas.
        let plan = prompts::plan_text(&ctx.req, &ctx.plan);
        let calls = texts.chunks(PAGES_PER_CALL).map(|chunk| {
            let prompt = format!("{plan}\nAnúncios:\n{}", chunk.join("\n"));
            self.verdict(ctx, prompts::DETAILS, prompt, "details")
        });
        let mut out = prompts::Verdict::default();
        let mut last = None;
        for r in futures_util::future::join_all(calls).await {
            match r {
                Ok(v) => out.keep.extend(v.keep),
                Err(e) => last = Some(e),
            }
        }
        match last {
            Some(e) if out.keep.is_empty() => Err(e),
            _ => Ok((out, pages)),
        }
    }

    async fn page_text(self: &Arc<Self>, ctx: &Arc<Ctx>, site: &Site, agent: &str, url: &str) -> Result<String> {
        const MAX: usize = 4000;
        match site.stage.unwrap_or(Stage::D) {
            stage @ (Stage::A | Stage::B | Stage::C) => {
                let cookie = if stage == Stage::C { self.fetcher.cookie(&site.id) } else { None };
                let page = self.fetcher.get(stage, url, cookie.as_deref()).await?;
                if let Some(why) = fetch::blocked(&page) {
                    bail!("{why}");
                }
                Ok(parse::text_from_html(&page.body, MAX))
            }
            Stage::D => {
                let _slot = ctx.chrome.acquire().await?;
                let browser = self.browser().await?;
                let lease = browser.lease().await?;
                let tools = self.toolbox(ctx, agent, site, lease.page().clone());
                let msg = tools.navigate(url).await?;
                if !msg.starts_with("Página aberta") {
                    bail!("{}", msg.lines().next().unwrap_or_default());
                }
                lease.read_details(MAX).await
            }
        }
    }

    // ---------- agent ----------

    async fn run_agent(self: &Arc<Self>, ctx: &Arc<Ctx>, agent: &str, site: &Site) -> Result<String> {
        let _slot = ctx.chrome.acquire().await?;
        let browser = self.browser().await?;
        let lease = browser.lease().await?;
        let tools = self.toolbox(ctx, agent, site, lease.page().clone());
        let task = prompts::task(&ctx.req, &ctx.plan, std::slice::from_ref(site));
        let ai = &ctx.settings.ai;
        let (model, effort) = self.model_for(ai, Tier::Main).await;
        let backend = self.backend(ai, self.paths.work_dir.join(agent), &model, effort)?;
        match backend {
            Backend::Cli(run) => {
                self.mcp.register(agent, tools.clone());
                let mcp = Mcp { url: self.mcp.url_for(agent), token: self.mcp.token().to_string() };
                let r = cli::run_agent(run, prompts::SYSTEM, task, tools.clone(), mcp, ctx.cancel.clone()).await;
                self.mcp.unregister(agent);
                r
            }
            chat => match Self::chat(&chat, prompts::SYSTEM, true) {
                Some(session) => run_chat_agent(session, task, tools.clone(), ctx.settings.max_steps, ctx.cancel.clone()).await,
                None => Err(anyhow!("configuração de IA inválida")),
            },
        }
    }
}

/// Loja sem URL de busca nem API: só o agent consegue buscar.
fn needs_agent(site: &Site) -> bool {
    site.search_url.is_empty() && site.api.is_none()
}

fn terms(ctx: &Ctx, site: &Site) -> Vec<String> {
    // Os termos amplos ("pc gamer") trazem anúncios que só citam o modelo na descrição.
    let t: Vec<String> = ctx.plan.search_terms.iter().take(4).chain(ctx.plan.broad_terms.iter().take(2)).filter(|t| !t.trim().is_empty()).cloned().collect();
    let t = if t.is_empty() { vec![ctx.req.query.trim().to_string()] } else { t };
    let mut out: Vec<String> = Vec::new();
    for term in t.iter().map(|x| without_store_name(x, site)) {
        if !out.iter().any(|o| o.eq_ignore_ascii_case(&term)) {
            out.push(term);
        }
    }
    out
}

/// Buscas de uma loja: os termos do plano e, no fim, as versões mais curtas
/// dos termos cujos resultados não citam o modelo.
struct Queue {
    todo: VecDeque<(String, bool)>,
    seen: Vec<String>,
    retries: usize,
}

impl Queue {
    fn new(terms: Vec<String>, retries: usize) -> Self {
        Self { seen: terms.clone(), todo: terms.into_iter().map(|t| (t, false)).collect(), retries }
    }

    /// (termo, é uma busca mais curta)
    fn next(&mut self) -> Option<(String, bool)> {
        self.todo.pop_front()
    }

    fn after(&mut self, term: &str, retry: bool, product: &str, cards: &[Card]) {
        // Uma busca curta sem nenhum resultado pode encurtar de novo.
        if (retry && !cards.is_empty()) || self.retries == 0 {
            return;
        }
        if let Some(q) = parse::narrower_term(term, product, cards).filter(|q| !self.seen.iter().any(|s| s.eq_ignore_ascii_case(q))) {
            self.retries -= 1;
            self.seen.push(q.clone());
            self.todo.push_back((q, true));
        }
    }
}

/// Na loja da própria marca, o nome da marca casa com todo o catálogo e empurra
/// o modelo pedido para fora da primeira página ("XT Racer X-Pro" → "X-Pro").
fn without_store_name(term: &str, site: &Site) -> String {
    let key = |w: &str| parse::fold(w).trim_matches(|c: char| !c.is_alphanumeric()).to_string();
    let mut names: Vec<Vec<String>> = vec![site.name.split_whitespace().map(key).filter(|w| !w.is_empty()).collect()];
    if let Some(label) = site.domain.split('.').max_by_key(|l| l.len()).filter(|l| l.len() >= 4) {
        names.push(vec![key(label)]);
    }
    names.retain(|n| !n.is_empty());
    let words: Vec<&str> = term.split_whitespace().collect();
    let keys: Vec<String> = words.iter().map(|w| key(w)).collect();
    let mut kept = Vec::new();
    let mut i = 0;
    while i < words.len() {
        if let Some(n) = names.iter().find(|n| keys[i..].starts_with(n)) {
            i += n.len();
            continue;
        }
        kept.push(words[i]);
        i += 1;
    }
    let left = kept.join(" ");
    if left.chars().filter(|c| c.is_alphanumeric()).count() >= 2 {
        left
    } else {
        term.trim().to_string()
    }
}

fn vtex_url(site: &Site, term: &str) -> String {
    format!("{}/api/catalog_system/pub/products/search?ft={}&_from=0&_to=39", site.origin(), urlencoding::encode(term))
}

fn merge(all: &mut Vec<Card>, cards: Vec<Card>) {
    let mut seen: HashSet<String> = all.iter().map(|c| crate::tools::dedupe_key(&c.url, &c.title)).collect();
    for c in cards {
        if seen.insert(crate::tools::dedupe_key(&c.url, &c.title)) {
            all.push(c);
        }
    }
}

fn snipr_rand(lo: u64, hi: u64) -> u64 {
    use rand::Rng;
    rand::thread_rng().gen_range(lo..=hi)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::{AuthMode, Paths, Provider},
        types::ConditionFilter,
    };

    #[test]
    fn tira_o_nome_da_loja_do_termo() {
        let site = |name: &str, domain: &str| -> Site { serde_json::from_value(serde_json::json!({"id": "x", "name": name, "domain": domain})).unwrap() };
        let xt = site("XT Racer", "xtracer.com.br");
        assert_eq!(without_store_name("XT Racer X-Pro", &xt), "X-Pro");
        assert_eq!(without_store_name("cadeira xt racer modelo x-pro", &xt), "cadeira modelo x-pro");
        assert_eq!(without_store_name("cadeira XTRacer X-Pro", &xt), "cadeira X-Pro");
        assert_eq!(without_store_name("XT Racer", &xt), "XT Racer", "sem sobra, fica o termo original");
        let ali = site("AliExpress", "pt.aliexpress.com");
        assert_eq!(without_store_name("monitor pt 27", &ali), "monitor pt 27");
        assert_eq!(without_store_name("cadeira XT Racer X-Pro", &site("KaBuM!", "kabum.com.br")), "cadeira XT Racer X-Pro");
    }

    /// Diagnóstico: lê as lojas com um plano fixo, sem IA, e mostra o que iria ao filtro.
    ///   SNIPR_TEST_SETTINGS=<settings.json> SNIPR_TEST_SITES=kabum,x-dt3-com cargo test -p snipr-core --release coleta_sem_ia -- --ignored --nocapture
    #[tokio::test]
    #[ignore]
    async fn coleta_sem_ia() {
        let _ = tracing_subscriber::fmt().with_env_filter(std::env::var("RUST_LOG").unwrap_or_else(|_| "warn".into())).try_init();
        let tmp = std::env::temp_dir().join("snipr-collect-test");
        std::fs::create_dir_all(&tmp).unwrap();
        if let Ok(src) = std::env::var("SNIPR_TEST_SETTINGS") {
            std::fs::copy(src, tmp.join("settings.json")).unwrap();
        }
        let paths = Paths { config_file: tmp.join("settings.json"), profile_dir: tmp.join("profile"), work_dir: tmp.join("agents") };
        let engine = Engine::with_paths(paths).await.unwrap();
        let settings = engine.settings();
        // SNIPR_TEST_PLAN='{"product":"console","alternatives":["PlayStation 5","PS5"],"search_terms":["playstation 5","ps5"]}'
        let plan = std::env::var("SNIPR_TEST_PLAN").ok().and_then(|p| serde_json::from_str(&p).ok()).unwrap_or_else(|| SearchPlan {
            product: "cadeira".into(),
            alternatives: vec!["ThunderX3 Core XXL".into(), "DT3 Royce".into(), "XT Office X-Pro".into(), "DT3 Forst".into()],
            search_terms: vec!["cadeira ThunderX3 Core XXL".into(), "cadeira DT3 Royce".into(), "cadeira XT Office X-Pro".into(), "cadeira DT3 Forst".into()],
            ..Default::default()
        });
        let only = std::env::var("SNIPR_TEST_SITES").unwrap_or_default();
        let sites: Vec<Site> = settings.all_sites().into_iter().filter(|s| settings.enabled_sites.contains(&s.id)).filter(|s| only.is_empty() || only.split(',').any(|o| s.id == o)).collect();
        let (tx, mut rx) = mpsc::unbounded_channel();
        tokio::spawn(async move { while rx.recv().await.is_some() {} });
        let req = SearchRequest { query: "cadeira".into(), condition: ConditionFilter::Any, sites: vec![], max_price: None, plan: None };
        let ctx = Arc::new(Ctx {
            job: "t".into(),
            sink: Arc::new(Sink { job: "t".into(), events: tx.clone(), listings: Default::default(), filter: req.condition, max_price: None }),
            req,
            plan,
            chrome: Arc::new(Semaphore::new(4)),
            all_sites: settings.all_sites(),
            settings,
            events: tx,
            cancel: CancellationToken::new(),
            read_pages: Default::default(),
            memo: None,
        });
        let words = parse::tokens(&format!("{} {} {}", ctx.plan.product, ctx.plan.alternatives.join(" "), ctx.plan.search_terms.join(" ")));
        let mut set = tokio::task::JoinSet::new();
        for site in sites {
            let (me, ctx, words) = (engine.clone(), ctx.clone(), words.clone());
            set.spawn(async move {
                let t = std::time::Instant::now();
                let r = me.collect(&ctx, &site, &site.id).await;
                let mut out = format!("\n=== {} ({:?}) termos {:?} em {:.1}s\n", site.name, site.stage, terms(&ctx, &site), t.elapsed().as_secs_f64());
                match r {
                    Ok(Collected::NeedsAgent) => out.push_str("  -> agent\n"),
                    Ok(Collected::Cards(cards, stage)) => {
                        let groups = parse::plan_groups(&ctx.plan.product, &ctx.plan.alternatives, &ctx.plan.search_terms);
                        let refs: Vec<&Card> = cards.iter().collect();
                        let short: Vec<String> = parse::shortlist(&refs, &groups, &ctx.plan.product, true, &words, MAX_CANDIDATES).iter().map(|c| c.url.clone() + &c.title).collect();
                        out.push_str(&format!("  etapa {} · {} cards · {} ao filtro\n", stage.label(), cards.len(), short.len()));
                        for (i, c) in cards.iter().enumerate() {
                            let judged = short.contains(&(c.url.clone() + &c.title));
                            out.push_str(&format!("  {:>2} {} {:?} {}\n", i, if judged { "J" } else { " " }, parse::sorted_prices(c).first(), c.title.chars().take(90).collect::<String>()));
                        }
                    }
                    Err(e) => out.push_str(&format!("  erro: {e:#}\n")),
                }
                out
            });
        }
        while let Some(Ok(s)) = set.join_next().await {
            println!("{s}");
        }
        engine.shutdown().await;
    }

    /// Busca completa com a IA do login do Claude Code (usa a IA):
    ///   SNIPR_TEST_PROFILE=<cópia do perfil> SNIPR_TEST_SITES=kabum,terabyte cargo test -p snipr-core --release busca_ponta_a_ponta -- --ignored --nocapture
    #[tokio::test]
    #[ignore]
    async fn busca_ponta_a_ponta() {
        let _ = tracing_subscriber::fmt().with_env_filter(std::env::var("RUST_LOG").unwrap_or_else(|_| "warn".into())).try_init();
        let tmp = std::env::temp_dir().join("snipr-search-test");
        std::fs::create_dir_all(&tmp).unwrap();
        let profile = std::env::var("SNIPR_TEST_PROFILE").map(std::path::PathBuf::from).unwrap_or_else(|_| tmp.join("profile"));
        let paths = Paths { config_file: tmp.join("settings.json"), profile_dir: profile, work_dir: tmp.join("agents") };
        // SNIPR_TEST_SETTINGS=<settings.json>: usa as lojas e etapas de um perfil real.
        let given = std::env::var("SNIPR_TEST_SETTINGS").ok();
        if let Some(src) = &given {
            std::fs::copy(src, &paths.config_file).unwrap();
        } else {
            let mut s = Settings::default();
            s.ai.mode = AuthMode::Login;
            s.ai.login_provider = Provider::Anthropic;
            paths.save_settings(&s).unwrap();
        }
        let engine = Engine::with_paths(paths).await.unwrap();
        let default_sites = if given.is_some() { String::new() } else { "kabum,terabyte,magalu,mercadolivre".into() };
        let sites: Vec<String> =
            std::env::var("SNIPR_TEST_SITES").unwrap_or(default_sites).split(',').filter(|s| !s.is_empty()).map(String::from).collect();
        let query = std::env::var("SNIPR_TEST_QUERY").unwrap_or_else(|_| "monitor 27 polegadas 144hz até 1200".into());
        let (tx, mut rx) = mpsc::unbounded_channel();
        let started = std::time::Instant::now();
        engine
            .start_search(SearchRequest { query, condition: ConditionFilter::Any, sites, max_price: None, plan: None }, tx)
            .unwrap();
        let (mut tokens, mut cost, mut calls) = (0u64, 0f64, 0);
        while let Some(ev) = rx.recv().await {
            let t = started.elapsed().as_secs_f64();
            match ev {
                SearchEvent::Planned { plan, .. } => println!("{t:5.1}s plano: {:?} | termos: {:?}", plan.product, plan.search_terms),
                SearchEvent::Log { agent, message, .. } => println!("{t:5.1}s [{agent}] {message}"),
                SearchEvent::Listing { listing, .. } => {
                    println!("{t:5.1}s   R$ {:>9.2} {:?} {} — {}", listing.price, listing.condition, listing.title.chars().take(70).collect::<String>(), listing.site_name)
                }
                SearchEvent::Usage { usage, .. } => {
                    calls += 1;
                    tokens += usage.input_tokens + usage.output_tokens;
                    cost += usage.cost_usd.unwrap_or(0.0);
                    println!("{t:5.1}s uso: {} entrada / {} saída, US$ {:.4} ({:?})", usage.input_tokens, usage.output_tokens, usage.cost_usd.unwrap_or(0.0), usage.model);
                }
                SearchEvent::AgentFinished { agent, ok, summary, .. } => println!("{t:5.1}s [{agent}] {} {summary}", if ok { "✓" } else { "✗" }),
                SearchEvent::Finished { total, .. } => {
                    println!("fim: {total} anúncio(s) em {t:.1}s | {calls} chamada(s) à IA, {tokens} tokens, US$ {cost:.4}");
                    break;
                }
                _ => {}
            }
        }
        engine.shutdown().await;
    }
}
