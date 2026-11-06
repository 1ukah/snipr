//! Ferramentas expostas ao agente. As mesmas definições servem para:
//! - o loop próprio (API Anthropic / OpenAI-compatível)
//! - o servidor MCP (Claude Code, ou qualquer cliente MCP no futuro)

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
use snipr_browser::{ChallengeOutcome, NavInfo, Page};
use snipr_llm::ToolSpec;
use tokio::sync::{mpsc, oneshot};

use crate::{
    secrets,
    sites::Site,
    types::{Condition, ConditionFilter, Listing, SearchEvent},
};

const MAX_OUTPUT: usize = 12_000;

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

pub struct ToolBox {
    pub job: String,
    pub agent: String,
    page: Arc<Page>,
    sites: Vec<Site>,
    all_sites: Vec<Site>,
    filter: ConditionFilter,
    max_price: Option<f64>,
    credential_domains: Vec<String>,
    events: mpsc::UnboundedSender<SearchEvent>,
    gates: Arc<HumanGates>,
    pub listings: Arc<Mutex<Vec<Listing>>>,
    finished: AtomicBool,
    summary: Mutex<String>,
    /// Serializa chamadas: o agente pode pedir ferramentas em paralelo,
    /// mas todas agem sobre a mesma aba.
    lock: tokio::sync::Mutex<()>,
}

pub struct ToolBoxInit {
    pub job: String,
    pub agent: String,
    pub page: Arc<Page>,
    pub sites: Vec<Site>,
    pub all_sites: Vec<Site>,
    pub filter: ConditionFilter,
    pub max_price: Option<f64>,
    pub credential_domains: Vec<String>,
    pub events: mpsc::UnboundedSender<SearchEvent>,
    pub gates: Arc<HumanGates>,
    pub listings: Arc<Mutex<Vec<Listing>>>,
}

impl ToolBox {
    pub fn new(i: ToolBoxInit) -> Self {
        Self {
            job: i.job,
            agent: i.agent,
            page: i.page,
            sites: i.sites,
            all_sites: i.all_sites,
            filter: i.filter,
            max_price: i.max_price,
            credential_domains: i.credential_domains,
            events: i.events,
            gates: i.gates,
            listings: i.listings,
            finished: AtomicBool::new(false),
            summary: Mutex::new(String::new()),
            lock: tokio::sync::Mutex::new(()),
        }
    }

    pub fn log(&self, message: impl Into<String>) {
        let _ = self.events.send(SearchEvent::Log {
            job: self.job.clone(),
            agent: self.agent.clone(),
            message: message.into(),
        });
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
              "Abre direto a página de resultados de busca de um site atribuído a você. Prefira isto a navegar pela home.",
              json!({ "type": "object", "properties": {
                  "site_id": { "type": "string", "description": "id do site (ex.: mercadolivre, olx)" },
                  "query": { "type": "string", "description": "termo de busca curto" }
              }, "required": ["site_id", "query"], "additionalProperties": false })),
            t("extract_listings",
              "Lê os cards de anúncios da página atual (título, preços, URL, texto extra). Barato e rápido: use logo após search_site.",
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
            t("type_secret",
              "Digita uma credencial guardada no cofre (usuário ou senha) do site atual no campo [ref]. Você nunca vê o valor. Só funciona no domínio dono da credencial.",
              json!({ "type": "object", "properties": {
                  "ref": { "type": "integer" },
                  "field": { "type": "string", "enum": ["username", "password"] },
                  "submit": { "type": "boolean" }
              }, "required": ["ref", "field"], "additionalProperties": false })),
            t("scroll", "Rola a página (carrega mais resultados em sites com rolagem infinita).",
              json!({ "type": "object", "properties": { "direction": { "type": "string", "enum": ["down", "up"] } }, "additionalProperties": false })),
            t("open_url", "Abre uma URL (ex.: página de um anúncio para conferir detalhes).",
              json!({ "type": "object", "properties": { "url": { "type": "string" } }, "required": ["url"], "additionalProperties": false })),
            t("read_text", "Lê o texto principal da página atual (útil na página de um anúncio).",
              json!({ "type": "object", "properties": { "max_chars": { "type": "integer" } }, "additionalProperties": false })),
            t("go_back", "Volta para a página anterior.", empty.clone()),
            t("report_listings",
              "Registra anúncios que correspondem ao produto pedido. Pode ser chamado várias vezes.",
              json!({ "type": "object", "properties": {
                  "site_id": { "type": "string" },
                  "listings": { "type": "array", "items": { "type": "object", "properties": {
                      "title": { "type": "string" },
                      "price": { "type": "number", "description": "preço à vista atual em reais, ex.: 1299.9" },
                      "condition": { "type": "string", "enum": ["new", "used", "unknown"] },
                      "url": { "type": "string" },
                      "seller": { "type": "string" },
                      "location": { "type": "string" },
                      "shipping": { "type": "string" },
                      "notes": { "type": "string" }
                  }, "required": ["title", "price", "condition", "url"] } }
              }, "required": ["site_id", "listings"], "additionalProperties": false })),
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
                let url = site.build_search_url(&q);
                let nav = self.navigate(&url).await?;
                Ok(nav)
            }
            "open_url" => {
                let url = s("url");
                if !(url.starts_with("https://") || url.starts_with("http://")) {
                    anyhow::bail!("URL inválida (precisa começar com http/https)");
                }
                self.navigate(&url).await
            }
            "extract_listings" => self.page.extract_listings(40).await,
            "snapshot" => {
                let f = s("filter");
                self.page.snapshot(150, if f.is_empty() { None } else { Some(f.as_str()) }).await
            }
            "read_text" => self.page.read_text(n("max_chars").unwrap_or(5000).min(10_000) as usize).await,
            "click" => self.page.click_ref(n("ref").ok_or_else(|| anyhow::anyhow!("ref ausente"))? as u32).await,
            "type_text" => {
                let r = n("ref").ok_or_else(|| anyhow::anyhow!("ref ausente"))? as u32;
                let submit = input.get("submit").and_then(Value::as_bool).unwrap_or(false);
                self.page.type_into(r, &s("text"), submit).await?;
                Ok(format!("Digitado{}.", if submit { " e enviado" } else { "" }))
            }
            "type_secret" => self.type_secret(input).await,
            "scroll" => {
                self.page.scroll(s("direction") != "up").await?;
                Ok("Página rolada.".into())
            }
            "go_back" => {
                let nav = self.page.go_back().await?;
                Ok(format!("Voltou para {} — {}", nav.url, nav.title))
            }
            "report_listings" => self.report(input),
            "request_human" => self.ask_human(&s("reason"), None).await,
            "finish" => {
                *self.summary.lock() = s("summary");
                self.finished.store(true, Ordering::SeqCst);
                Ok("Encerrado. Não chame mais ferramentas.".into())
            }
            other => anyhow::bail!("ferramenta desconhecida: {other}"),
        }
    }

    async fn navigate(&self, url: &str) -> anyhow::Result<String> {
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
                     Não insista: siga para o próximo site e informe no resumo que {host} bloqueou."
                ));
            }
        }
        if let ChallengeOutcome::Unsolved(kind) = &nav.challenge {
            let host = host_of(&nav.url);
            let reason = format!("Desafio anti-bot ({kind}) em {host} não passou sozinho. Resolva na janela que abriu e clique em 'Resolvido'.");
            let r = self.ask_human(&reason, Some(host)).await?;
            nav = self.page.info(ChallengeOutcome::None).await?;
            if let Some(k) = self.page.detect_challenge().await {
                return Ok(format!("{r}\nO site continua bloqueado ({k}). Siga para o próximo site."));
            }
        }
        Ok(format!(
            "Página aberta: {}\nTítulo: {}\nDesafio anti-bot: {}\nPróximo passo sugerido: extract_listings.",
            nav.url,
            nav.title,
            nav.challenge.describe()
        ))
    }

    async fn ask_human(&self, reason: &str, site: Option<String>) -> anyhow::Result<String> {
        let _ = self.events.send(SearchEvent::NeedsHuman {
            job: self.job.clone(),
            agent: self.agent.clone(),
            site,
            reason: reason.to_string(),
        });
        let shown = self.page.set_visible(true).await;
        if let Err(e) = &shown {
            self.log(format!("não foi possível mostrar a janela: {e}"));
        }
        let r = self.gates.wait(&self.agent, Duration::from_secs(300)).await;
        let _ = self.page.set_visible(false).await;
        let _ = self.events.send(SearchEvent::HumanResolved { job: self.job.clone(), agent: self.agent.clone() });
        Ok(match r {
            Some(true) => "O usuário concluiu a ação manual. Continue.".into(),
            Some(false) => "O usuário preferiu pular. Siga para o próximo site.".into(),
            None => "Sem resposta do usuário (tempo esgotado). Siga para o próximo site.".into(),
        })
    }

    async fn type_secret(&self, input: &Value) -> anyhow::Result<String> {
        let r = input.get("ref").and_then(Value::as_u64).ok_or_else(|| anyhow::anyhow!("ref ausente"))? as u32;
        let field = input.get("field").and_then(Value::as_str).unwrap_or_default();
        if field != "username" && field != "password" {
            anyhow::bail!("field deve ser 'username' ou 'password'");
        }
        let host = host_of(&self.page.url().await?);
        // A credencial só pode ser digitada no domínio dono dela: protege contra
        // páginas maliciosas que tentem induzir o agente a vazar a senha.
        let domain = self
            .credential_domains
            .iter()
            .find(|d| host == **d || host.ends_with(&format!(".{d}")))
            .ok_or_else(|| anyhow::anyhow!("não há credencial cadastrada para {host}. Use request_human para o usuário logar."))?;
        let value = secrets::reveal(&secrets::site_key(domain, field))
            .ok_or_else(|| anyhow::anyhow!("credencial de {domain} não encontrada no cofre"))?;
        let submit = input.get("submit").and_then(Value::as_bool).unwrap_or(false);
        self.page.type_into(r, &value, submit).await?;
        Ok(format!("{field} de {domain} digitado{}.", if submit { " e enviado" } else { "" }))
    }

    fn report(&self, input: &Value) -> anyhow::Result<String> {
        let site_id = input.get("site_id").and_then(Value::as_str).unwrap_or_default();
        let site = self.all_sites.iter().find(|s| s.id == site_id);
        let site_name = site.map(|s| s.name.clone()).unwrap_or_else(|| site_id.to_string());
        let items = input.get("listings").and_then(Value::as_array).cloned().unwrap_or_default();
        let (mut added, mut skipped) = (0, Vec::new());
        for it in items {
            let title = it["title"].as_str().unwrap_or_default().trim().to_string();
            let url = it["url"].as_str().unwrap_or_default().trim().to_string();
            let price = parse_price(&it["price"]);
            let condition = match it["condition"].as_str() {
                Some("new") => Condition::New,
                Some("used") => Condition::Used,
                _ => Condition::Unknown,
            };
            let Some(price) = price.filter(|p| p.is_finite() && *p > 0.0) else {
                skipped.push(format!("'{title}': preço inválido"));
                continue;
            };
            if title.is_empty() || !url.starts_with("http") {
                skipped.push(format!("'{title}': título/URL inválidos"));
                continue;
            }
            match (self.filter, condition) {
                (ConditionFilter::New, Condition::Used) | (ConditionFilter::Used, Condition::New) => {
                    skipped.push(format!("'{title}': condição diferente da pedida"));
                    continue;
                }
                _ => {}
            }
            if self.max_price.is_some_and(|m| price > m) {
                skipped.push(format!("'{title}': acima do preço máximo"));
                continue;
            }
            let key = dedupe_key(&url, &title);
            let mut all = self.listings.lock();
            if all.iter().any(|l| dedupe_key(&l.url, &l.title) == key) {
                continue;
            }
            let opt = |k: &str| it[k].as_str().map(str::trim).filter(|s| !s.is_empty()).map(String::from);
            let listing = Listing {
                id: uuid::Uuid::new_v4().to_string(),
                site_id: site_id.to_string(),
                site_name: site_name.clone(),
                title,
                price,
                currency: "BRL".into(),
                condition,
                url,
                seller: opt("seller"),
                location: opt("location"),
                shipping: opt("shipping"),
                notes: opt("notes"),
            };
            all.push(listing.clone());
            drop(all);
            added += 1;
            let _ = self.events.send(SearchEvent::Listing {
                job: self.job.clone(),
                agent: self.agent.clone(),
                listing: Box::new(listing),
            });
        }
        let mut out = format!("{added} anúncio(s) registrado(s).");
        if !skipped.is_empty() {
            out.push_str(&format!(" Ignorados: {}", skipped.join("; ")));
        }
        Ok(out)
    }

    pub fn assigned_sites(&self) -> &[Site] {
        &self.sites
    }
}

fn parse_price(v: &Value) -> Option<f64> {
    if let Some(f) = v.as_f64() {
        return Some(f);
    }
    // Aceita "R$ 1.299,90" caso o modelo mande texto.
    let s: String = v.as_str()?.chars().filter(|c| c.is_ascii_digit() || *c == ',' || *c == '.').collect();
    let s = if s.contains(',') { s.replace('.', "").replace(',', ".") } else { s };
    s.parse().ok()
}

/// Mesmo link + mesmo título = mesmo anúncio. Só o link não basta: em sites cujos
/// cards não expõem o link do item (ex.: Shopee), vários anúncios diferentes
/// chegam com o mesmo link de busca e seriam descartados como repetidos.
fn dedupe_key(url: &str, title: &str) -> String {
    let base = url.split(['?', '#']).next().unwrap_or(url).trim_end_matches('/').to_lowercase();
    let t: String = title.to_lowercase().split_whitespace().collect::<Vec<_>>().join(" ");
    format!("{base}|{t}")
}

fn host_of(u: &str) -> String {
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
        "type_secret" => "preenchendo credencial do cofre".into(),
        "scroll" => "rolando a página".into(),
        "read_text" => "lendo detalhes do anúncio".into(),
        "go_back" => "voltando".into(),
        "report_listings" => {
            let n = input.get("listings").and_then(Value::as_array).map(|a| a.len()).unwrap_or(0);
            format!("registrando {n} anúncio(s) de {}", s("site_id"))
        }
        "request_human" => format!("pedindo ajuda: {}", s("reason")),
        _ => return None,
    })
}
