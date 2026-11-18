//! Ferramentas do agent (lojas sem leitura automática). As mesmas definições servem para:
//! - o loop próprio (API Anthropic / OpenAI-compatível)
//! - o servidor MCP (Claude Code, ou qualquer cliente MCP no futuro)
//!
//! A navegação com tratamento de desafio, login e ajuda humana também é usada
//! pela leitura automática na etapa D.

use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};

use parking_lot::Mutex;
use serde_json::{json, Value};
use snipr_browser::{Card, ChallengeOutcome, NavInfo, Page};
use snipr_llm::ToolSpec;
use tokio::sync::{mpsc, oneshot};

use crate::{
    config::SessionState,
    offers, parse,
    sites::Site,
    types::{Condition, ConditionFilter, HumanKind, Listing, SearchEvent, Usage},
};

pub type SessionHook = Arc<dyn Fn(&str, bool) + Send + Sync>;

/// Limite de cada resposta de ferramenta: tudo o que volta para o modelo é
/// reenviado a cada passo seguinte.
const MAX_OUTPUT: usize = 5_000;
const MAX_CARDS: usize = 25;

/// Pontos onde o agente espera uma ação humana (desafio, login, 2FA).
#[derive(Default)]
pub struct HumanGates {
    waiting: Mutex<HashMap<String, oneshot::Sender<bool>>>,
}

impl HumanGates {
    pub async fn wait(&self, agent: &str, max: Duration) -> Option<bool> {
        let (tx, rx) = oneshot::channel();
        self.waiting.lock().insert(agent.to_string(), tx);
        let r = tokio::time::timeout(max, rx).await.ok().and_then(|r| r.ok());
        self.waiting.lock().remove(agent);
        r
    }

    pub fn resolve(&self, agent: &str, ok: bool) -> bool {
        self.waiting.lock().remove(agent).map(|tx| tx.send(ok).is_ok()).unwrap_or(false)
    }

    pub fn cancel_all(&self) {
        for (_, tx) in self.waiting.lock().drain() {
            let _ = tx.send(false);
        }
    }
}

/// Destino dos anúncios de uma busca: valida, descarta repetidos e emite eventos.
pub struct Sink {
    pub job: String,
    pub events: mpsc::UnboundedSender<SearchEvent>,
    pub listings: Arc<Mutex<Vec<Listing>>>,
    pub filter: ConditionFilter,
    pub max_price: Option<f64>,
}

impl Sink {
    pub fn log(&self, agent: &str, message: impl Into<String>) {
        let _ = self.events.send(SearchEvent::Log { job: self.job.clone(), agent: agent.to_string(), message: message.into() });
    }

    pub fn usage(&self, agent: Option<&str>, usage: Usage) {
        if usage.is_empty() {
            return;
        }
        let _ = self.events.send(SearchEvent::Usage { job: self.job.clone(), agent: agent.map(String::from), usage });
    }

    /// Registra o anúncio. `Ok(false)` = repetido; `Err` = motivo da recusa.
    pub fn accept(&self, agent: &str, l: Listing) -> Result<bool, String> {
        if !(l.price.is_finite() && l.price > 0.0) {
            return Err("preço inválido".into());
        }
        if l.title.trim().is_empty() || !l.url.starts_with("http") {
            return Err("título/URL inválidos".into());
        }
        match (self.filter, l.condition) {
            (ConditionFilter::New, Condition::Used) | (ConditionFilter::Used, Condition::New) => return Err("condição diferente da pedida".into()),
            _ => {}
        }
        if self.max_price.is_some_and(|m| l.price > m) {
            return Err("acima do preço máximo".into());
        }
        let key = dedupe_key(&l.url, &l.title);
        {
            let mut all = self.listings.lock();
            if all.iter().any(|x| dedupe_key(&x.url, &x.title) == key) {
                return Ok(false);
            }
            all.push(l.clone());
        }
        let _ = self.events.send(SearchEvent::Listing { job: self.job.clone(), agent: agent.to_string(), listing: Box::new(l) });
        Ok(true)
    }

    /// Troca um anúncio já registrado (mesmo `id`) pela versão completada.
    pub fn update(&self, agent: &str, l: Listing) {
        {
            let mut all = self.listings.lock();
            let Some(x) = all.iter_mut().find(|x| x.id == l.id) else { return };
            *x = l.clone();
        }
        let _ = self.events.send(SearchEvent::Updated { job: self.job.clone(), agent: agent.to_string(), listing: Box::new(l) });
    }
}

/// Monta o anúncio a partir de um card. `price` = preço escolhido.
pub fn listing_from_card(site: &Site, card: &Card, price: f64, condition: Condition, fallback_url: &str) -> Listing {
    let original = card.old.iter().filter_map(|p| parse::parse_brl(p)).fold(0.0f64, f64::max);
    let mut l = Listing {
        id: uuid::Uuid::new_v4().to_string(),
        site_id: site.id.clone(),
        site_name: site.name.clone(),
        title: card.title.trim().to_string(),
        price,
        currency: "BRL".into(),
        condition,
        url: if card.url.is_empty() { fallback_url.to_string() } else { card.url.clone() },
        seller: None,
        location: None,
        shipping: None,
        notes: None,
        original_price: Some(original).filter(|o| *o > price),
        international: card.intl,
        card_price: None,
        installments: None,
        interest_free: false,
        offers: Vec::new(),
    };
    offers::apply(&mut l, &offers::read(&card.text, false));
    l
}

pub struct ToolBox {
    pub agent: String,
    page: Arc<Page>,
    sites: Vec<Site>,
    all_sites: Vec<Site>,
    sessions: HashMap<String, SessionState>,
    on_session: SessionHook,
    gates: Arc<HumanGates>,
    sink: Arc<Sink>,
    /// Cards lidos até agora (o id é a posição + 1) e a página de cada um.
    cards: Mutex<Vec<(Card, String)>>,
    finished: AtomicBool,
    summary: Mutex<String>,
    /// Serializa chamadas: o agente pode pedir ferramentas em paralelo,
    /// mas todas agem sobre a mesma aba.
    lock: tokio::sync::Mutex<()>,
}

pub struct ToolBoxInit {
    pub agent: String,
    pub page: Arc<Page>,
    pub sites: Vec<Site>,
    pub all_sites: Vec<Site>,
    pub sessions: HashMap<String, SessionState>,
    pub on_session: SessionHook,
    pub gates: Arc<HumanGates>,
    pub sink: Arc<Sink>,
}

impl ToolBox {
    pub fn new(i: ToolBoxInit) -> Self {
        Self {
            agent: i.agent,
            page: i.page,
            sites: i.sites,
            all_sites: i.all_sites,
            sessions: i.sessions,
            on_session: i.on_session,
            gates: i.gates,
            sink: i.sink,
            cards: Mutex::new(Vec::new()),
            finished: AtomicBool::new(false),
            summary: Mutex::new(String::new()),
            lock: tokio::sync::Mutex::new(()),
        }
    }

    pub fn log(&self, message: impl Into<String>) {
        self.sink.log(&self.agent, message);
    }

    pub fn usage(&self, usage: Usage) {
        self.sink.usage(Some(&self.agent), usage);
    }

    pub fn is_finished(&self) -> bool {
        self.finished.load(Ordering::SeqCst)
    }

    pub fn summary(&self) -> String {
        self.summary.lock().clone()
    }

    pub fn specs() -> Vec<ToolSpec> {
        let t = |name: &str, description: &str, schema: Value| ToolSpec {
            name: name.into(),
            description: description.into(),
            schema,
        };
        let empty = json!({ "type": "object", "properties": {}, "additionalProperties": false });
        vec![
            t("search_site",
              "Abre direto a página de resultados de busca da loja. Prefira isto a navegar pela home.",
              json!({ "type": "object", "properties": {
                  "site_id": { "type": "string", "description": "id da loja" },
                  "query": { "type": "string", "description": "termo de busca curto" }
              }, "required": ["site_id", "query"], "additionalProperties": false })),
            t("extract_listings",
              "Lê os cards de anúncios da página atual. Cada card ganha um id (#n) para report_listings. Preços do menor para o maior.",
              empty.clone()),
            t("snapshot",
              "Lista elementos interativos visíveis com referências [n] para click/type_text. Use só quando precisar interagir. 'filter' restringe por texto.",
              json!({ "type": "object", "properties": {
                  "filter": { "type": "string", "description": "texto para filtrar elementos (opcional)" }
              }, "additionalProperties": false })),
            t("click", "Clica no elemento [ref] do último snapshot.",
              json!({ "type": "object", "properties": { "ref": { "type": "integer" } }, "required": ["ref"], "additionalProperties": false })),
            t("type_text", "Digita texto no campo [ref] do último snapshot (substitui o conteúdo). submit=true aperta Enter.",
              json!({ "type": "object", "properties": {
                  "ref": { "type": "integer" }, "text": { "type": "string" }, "submit": { "type": "boolean" }
              }, "required": ["ref", "text"], "additionalProperties": false })),
            t("scroll", "Rola a página (carrega mais resultados em sites com rolagem infinita).",
              json!({ "type": "object", "properties": { "direction": { "type": "string", "enum": ["down", "up"] } }, "additionalProperties": false })),
            t("open_url", "Abre uma URL (ex.: página de um anúncio para conferir detalhes).",
              json!({ "type": "object", "properties": { "url": { "type": "string" } }, "required": ["url"], "additionalProperties": false })),
            t("read_text", "Lê o texto da página atual, com seções recolhidas (especificações, \"ver mais\") abertas.",
              json!({ "type": "object", "properties": { "max_chars": { "type": "integer" } }, "additionalProperties": false })),
            t("go_back", "Volta para a página anterior.", empty.clone()),
            t("report_listings",
              "Registra anúncios que atendem ao plano. Use o id do card; price só se o primeiro preço do card não for o à vista atual.",
              json!({ "type": "object", "properties": {
                  "site_id": { "type": "string" },
                  "items": { "type": "array", "items": { "type": "object", "properties": {
                      "id": { "type": "integer", "description": "id do card (#n) de extract_listings" },
                      "condition": { "type": "string", "enum": ["new", "used", "unknown"] },
                      "price": { "type": "number", "description": "preço à vista atual em reais, só para corrigir o do card" },
                      "title": { "type": "string", "description": "só para anúncio fora dos cards" },
                      "url": { "type": "string", "description": "só para anúncio fora dos cards" }
                  }, "required": ["condition"] } }
              }, "required": ["site_id", "items"], "additionalProperties": false })),
            t("request_human",
              "Pede ajuda ao usuário (ex.: login com 2FA, captcha que não passou). A janela do navegador é mostrada e você espera até ele terminar.",
              json!({ "type": "object", "properties": { "reason": { "type": "string" } }, "required": ["reason"], "additionalProperties": false })),
            t("finish", "Encerra seu trabalho com um resumo curto do que encontrou.",
              json!({ "type": "object", "properties": { "summary": { "type": "string" } }, "required": ["summary"], "additionalProperties": false })),
        ]
    }

    /// Executa uma ferramenta. Nunca devolve Err: erros viram texto para o modelo.
    pub async fn call(&self, name: &str, input: &Value) -> (String, bool) {
        let _guard = self.lock.lock().await;
        let name = name.strip_prefix("mcp__snipr__").unwrap_or(name);
        if let Some(desc) = describe(name, input) {
            self.log(desc);
        }
        match self.dispatch(name, input).await {
            Ok(out) => (truncate(out), false),
            Err(e) => {
                let msg = format!("{e:#}");
                self.log(format!("⚠ {name}: {msg}"));
                (truncate(msg), true)
            }
        }
    }

    async fn dispatch(&self, name: &str, input: &Value) -> anyhow::Result<String> {
        let s = |k: &str| input.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
        let n = |k: &str| input.get(k).and_then(Value::as_u64);
        match name {
            "search_site" => {
                let site_id = s("site_id");
                let site = self
                    .all_sites
                    .iter()
                    .find(|x| x.id == site_id)
                    .ok_or_else(|| anyhow::anyhow!("site desconhecido: {site_id}"))?;
                let q = s("query");
                if q.trim().is_empty() {
                    anyhow::bail!("query vazia");
                }
                if site.search_url.is_empty() {
                    let nav = self.navigate(&site.home()).await?;
                    return Ok(format!(
                        "{nav}\nEsta loja não tem URL de busca conhecida. Use snapshot com filter \"busca\" ou \"pesquisar\" para achar o campo de busca e type_text com submit=true para buscar \"{q}\"."
                    ));
                }
                self.navigate(&site.build_search_url(&q)).await
            }
            "open_url" => {
                let url = s("url");
                if !(url.starts_with("https://") || url.starts_with("http://")) {
                    anyhow::bail!("URL inválida (precisa começar com http/https)");
                }
                self.navigate(&url).await
            }
            "extract_listings" => self.extract().await,
            "snapshot" => {
                let f = s("filter");
                self.page.snapshot(80, if f.is_empty() { None } else { Some(f.as_str()) }).await
            }
            "read_text" => self.page.read_details(n("max_chars").unwrap_or(3000).min(6000) as usize).await,
            "click" => self.page.click_ref(n("ref").ok_or_else(|| anyhow::anyhow!("ref ausente"))? as u32).await,
            "type_text" => {
                let r = n("ref").ok_or_else(|| anyhow::anyhow!("ref ausente"))? as u32;
                let submit = input.get("submit").and_then(Value::as_bool).unwrap_or(false);
                self.page.type_into(r, &s("text"), submit).await?;
                Ok(format!("Digitado{}.", if submit { " e enviado" } else { "" }))
            }
            "scroll" => {
                self.page.scroll(s("direction") != "up").await?;
                Ok("Página rolada.".into())
            }
            "go_back" => {
                let nav = self.page.go_back().await?;
                Ok(format!("Voltou para {} — {}", nav.url, nav.title))
            }
            "report_listings" => self.report(input),
            "request_human" => self.ask_human(HumanKind::Other, &s("reason"), None).await,
            "finish" => {
                *self.summary.lock() = s("summary");
                self.finished.store(true, Ordering::SeqCst);
                Ok("Encerrado. Não chame mais ferramentas.".into())
            }
            other => anyhow::bail!("ferramenta desconhecida: {other}"),
        }
    }

    async fn extract(&self) -> anyhow::Result<String> {
        let found = self.page.extract_cards(MAX_CARDS).await?;
        let here = self.page.url().await.unwrap_or_default();
        if found.is_empty() {
            return Ok(format!("Nenhum card com preço encontrado em {here}. Tente snapshot, scroll ou read_text."));
        }
        let mut cards = self.cards.lock();
        let mut out = format!("URL: {here}\n");
        for c in found {
            // Mesmo card de uma leitura anterior: reaproveita o id.
            let id = match cards.iter().position(|(x, _)| x.url == c.url && x.title == c.title) {
                Some(i) => i + 1,
                None => {
                    cards.push((c.clone(), here.clone()));
                    cards.len()
                }
            };
            let prices = parse::sorted_prices(&c).into_iter().map(parse::format_brl).collect::<Vec<_>>().join(" / ");
            let old = if c.old.is_empty() { String::new() } else { format!(" (riscado {})", c.old.join(", ")) };
            let intl = if c.intl { " | INTERNACIONAL" } else { "" };
            let url = if c.url.is_empty() { "SEM LINK" } else { c.url.as_str() };
            let extra: String = c.extra.chars().take(100).collect();
            out.push_str(&format!("#{id} | {prices}{old}{intl} | {} | {url} | {extra}\n", c.title));
        }
        Ok(out)
    }

    /// Abre a URL tratando bloqueio, desafio anti-bot e login (pede ajuda ao
    /// usuário quando preciso). Devolve um resumo para o modelo.
    pub async fn navigate(&self, url: &str) -> anyhow::Result<String> {
        let mut nav: NavInfo = self.page.goto(url).await?;
        if nav.challenge == ChallengeOutcome::Unsolved("blocked".into()) {
            // Bloqueio temporário na borda (403/429, "tente novamente em 1 minuto"):
            // uma pessoa não resolve isso clicando. Esperamos e tentamos uma vez.
            let host = host_of(&nav.url);
            self.log(format!("{host} bloqueou temporariamente (403/limite de acessos); aguardando ~1 min para tentar de novo"));
            tokio::time::sleep(Duration::from_secs(65)).await;
            nav = self.page.goto(url).await?;
            if nav.challenge == ChallengeOutcome::Unsolved("blocked".into()) {
                return Ok(format!(
                    "{host} está bloqueando o acesso temporariamente (erro 403 / limite de acessos), mesmo após esperar. \
                     Não insista: informe no resumo que {host} bloqueou."
                ));
            }
        }
        if let ChallengeOutcome::Unsolved(kind) = &nav.challenge {
            let host = host_of(&nav.url);
            let reason = format!("Desafio anti-bot ({kind}) em {host} não passou sozinho.");
            let r = self.ask_human(HumanKind::Challenge, &reason, self.site_for(&host).map(|s| s.id.clone())).await?;
            nav = self.page.info(ChallengeOutcome::None).await?;
            if let Some(k) = self.page.detect_challenge().await {
                return Ok(format!("{r}\nA loja continua bloqueada ({k})."));
            }
        }
        if self.login_wall().await {
            let host = host_of(&nav.url);
            let site = self.site_for(&host).cloned();
            let site_id = site.as_ref().map(|s| s.id.clone());
            let expired = site_id.as_ref().is_some_and(|id| self.sessions.get(id) == Some(&SessionState::Ok));
            if let Some(id) = &site_id {
                (self.on_session)(id, false);
            }
            let kind = if expired { HumanKind::Expired } else { HumanKind::Login };
            let r = self.ask_human(kind, &format!("{host} pede login."), site_id.clone()).await?;
            if r.starts_with("O usuário concluiu") {
                if let Some(id) = &site_id {
                    (self.on_session)(id, true);
                }
                nav = self.page.goto(url).await?;
                if self.login_wall().await {
                    return Ok(format!("{host} continua pedindo login."));
                }
            } else {
                return Ok(r);
            }
        }
        Ok(format!(
            "Página aberta: {}\nTítulo: {}\nDesafio anti-bot: {}\nPróximo passo sugerido: extract_listings.",
            nav.url,
            nav.title,
            nav.challenge.describe()
        ))
    }

    fn site_for(&self, host: &str) -> Option<&Site> {
        self.all_sites.iter().find(|s| s.matches_host(host))
    }

    async fn login_wall(&self) -> bool {
        const JS: &str = r#"(() => {
            const path = (location.pathname + location.search).toLowerCase();
            if (/(^|[\/_.?=-])(login|signin|sign-in|entrar|acesso|auth|account\/login)([\/_.?=&-]|$)/.test(path)) return true;
            const pw = [...document.querySelectorAll('input[type=password]')].some((e) => {
                const r = e.getBoundingClientRect();
                const st = getComputedStyle(e);
                return r.width > 0 && r.height > 0 && st.visibility !== 'hidden' && st.display !== 'none';
            });
            return pw;
        })()"#;
        self.page.eval(JS).await.ok().and_then(|v| v.as_bool()).unwrap_or(false)
    }

    async fn ask_human(&self, kind: HumanKind, reason: &str, site: Option<String>) -> anyhow::Result<String> {
        let _ = self.sink.events.send(SearchEvent::NeedsHuman {
            job: self.sink.job.clone(),
            agent: self.agent.clone(),
            site,
            kind,
            reason: reason.to_string(),
        });
        let shown = self.page.set_visible(true).await;
        if let Err(e) = &shown {
            self.log(format!("não foi possível mostrar a janela: {e}"));
        }
        let r = self.gates.wait(&self.agent, Duration::from_secs(300)).await;
        let _ = self.page.set_visible(false).await;
        let _ = self.sink.events.send(SearchEvent::HumanResolved { job: self.sink.job.clone(), agent: self.agent.clone() });
        Ok(match r {
            Some(true) => "O usuário concluiu a ação manual. Continue.".into(),
            Some(false) => "O usuário preferiu pular esta loja.".into(),
            None => "Sem resposta do usuário (tempo esgotado).".into(),
        })
    }

    fn report(&self, input: &Value) -> anyhow::Result<String> {
        let site_id = input.get("site_id").and_then(Value::as_str).unwrap_or_default();
        let site = self.all_sites.iter().find(|s| s.id == site_id).or(self.sites.first()).ok_or_else(|| anyhow::anyhow!("loja desconhecida"))?;
        let items = input.get("items").or_else(|| input.get("listings")).and_then(Value::as_array).cloned().unwrap_or_default();
        let (mut added, mut skipped) = (0, Vec::new());
        for it in items {
            let condition = match it["condition"].as_str() {
                Some("new") => Condition::New,
                Some("used") => Condition::Used,
                _ => Condition::Unknown,
            };
            let given_price = parse_price(&it["price"]).filter(|p| p.is_finite() && *p > 0.0);
            let listing = match it["id"].as_u64() {
                Some(id) => {
                    let Some((card, page)) = self.cards.lock().get((id as usize).wrapping_sub(1)).cloned() else {
                        skipped.push(format!("#{id}: id desconhecido"));
                        continue;
                    };
                    let Some(price) = given_price.or_else(|| parse::sorted_prices(&card).first().copied()) else {
                        skipped.push(format!("#{id}: sem preço"));
                        continue;
                    };
                    let mut l = listing_from_card(site, &card, price, condition, &page);
                    if let Some(u) = it["url"].as_str().filter(|u| u.starts_with("http")) {
                        l.url = u.to_string();
                    }
                    l
                }
                None => {
                    let title = it["title"].as_str().unwrap_or_default().to_string();
                    let url = it["url"].as_str().unwrap_or_default().to_string();
                    let card = Card { title, url, ..Default::default() };
                    listing_from_card(site, &card, given_price.unwrap_or(0.0), condition, "")
                }
            };
            let label = listing.title.chars().take(50).collect::<String>();
            match self.sink.accept(&self.agent, listing) {
                Ok(true) => added += 1,
                Ok(false) => {}
                Err(why) => skipped.push(format!("'{label}': {why}")),
            }
        }
        let mut out = format!("{added} anúncio(s) registrado(s).");
        if !skipped.is_empty() {
            out.push_str(&format!(" Ignorados: {}", skipped.join("; ")));
        }
        Ok(out)
    }
}

fn parse_price(v: &Value) -> Option<f64> {
    if let Some(f) = v.as_f64() {
        return Some(f);
    }
    // Aceita "R$ 1.299,90" caso o modelo mande texto.
    v.as_str().and_then(parse::parse_brl)
}

/// Mesmo link + mesmo título = mesmo anúncio. Só o link não basta: em sites cujos
/// cards não expõem o link do item (ex.: Shopee), vários anúncios diferentes
/// chegam com o mesmo link de busca e seriam descartados como repetidos.
pub fn dedupe_key(url: &str, title: &str) -> String {
    let base = url.split(['?', '#']).next().unwrap_or(url).trim_end_matches('/').to_lowercase();
    let t: String = title.to_lowercase().split_whitespace().collect::<Vec<_>>().join(" ");
    format!("{base}|{t}")
}

pub fn host_of(u: &str) -> String {
    url::Url::parse(u)
        .ok()
        .and_then(|x| x.host_str().map(|h| h.trim_start_matches("www.").to_string()))
        .unwrap_or_default()
}

fn truncate(mut s: String) -> String {
    if s.len() > MAX_OUTPUT {
        let mut cut = MAX_OUTPUT;
        while !s.is_char_boundary(cut) {
            cut -= 1;
        }
        s.truncate(cut);
        s.push_str("\n…(saída cortada)");
    }
    s
}

/// Linha curta para o painel de atividade.
fn describe(name: &str, input: &Value) -> Option<String> {
    let s = |k: &str| input.get(k).and_then(Value::as_str).unwrap_or_default();
    Some(match name {
        "search_site" => format!("buscando \"{}\" em {}", s("query"), s("site_id")),
        "open_url" => format!("abrindo {}", host_of(s("url"))),
        "extract_listings" => "lendo anúncios da página".into(),
        "snapshot" => "inspecionando a página".into(),
        "click" => "clicando".into(),
        "type_text" => "digitando".into(),
        "scroll" => "rolando a página".into(),
        "read_text" => "lendo detalhes do anúncio".into(),
        "go_back" => "voltando".into(),
        "report_listings" => {
            let n = input.get("items").or_else(|| input.get("listings")).and_then(Value::as_array).map(|a| a.len()).unwrap_or(0);
            format!("registrando {n} anúncio(s) de {}", s("site_id"))
        }
        "request_human" => format!("pedindo ajuda: {}", s("reason")),
        _ => return None,
    })
}
