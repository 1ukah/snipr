//! Buscas automáticas: a mesma busca, nas lojas escolhidas, a cada X minutos
//! ou todo dia num horário. O app precisa estar aberto.
//!
//! Para gastar poucos tokens, cada busca automática guarda o plano da
//! primeira execução e os vereditos da IA (`Memo`): nas execuções seguintes,
//! só anúncios novos vão para o filtro.
//!
//! Arquivos, na pasta dos ajustes:
//! - `schedules.json`: as buscas automáticas.
//! - `auto/<id>.json`: resultado da última execução.
//! - `auto/<id>.memo.json`: vereditos da IA.

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};

use anyhow::{anyhow, bail, Result};
use chrono::{Local, TimeZone};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, oneshot};
use ts_rs::TS;

use crate::{
    engine::{unix_now, Engine},
    history,
    search::Memo,
    tools::dedupe_key,
    types::{ConditionFilter, Listing, SearchEvent, SearchPlan, SearchRequest, Usage},
};

/// Menor intervalo aceito.
pub const MIN_MINUTES: u32 = 5;
/// De quanto em quanto tempo o Snipr confere se alguma busca deve rodar.
const TICK: Duration = Duration::from_secs(20);

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[ts(export)]
pub enum Every {
    /// A cada `minutes` minutos.
    Interval { minutes: u32 },
    /// Todo dia, no horário local.
    Daily { hour: u8, minute: u8 },
}

impl Default for Every {
    fn default() -> Self {
        Self::Interval { minutes: 60 }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[serde(default)]
#[ts(export)]
pub struct Schedule {
    /// Vazio ao criar.
    pub id: String,
    pub query: String,
    pub sites: Vec<String>,
    pub condition: ConditionFilter,
    pub max_price: Option<f64>,
    pub every: Every,
    pub enabled: bool,
    /// Plano da primeira execução, reusado nas seguintes.
    pub plan: Option<SearchPlan>,
    #[ts(type = "number | null")]
    pub last_run: Option<u64>,
    pub last_total: usize,
    /// Anúncios que não estavam na execução anterior.
    pub last_new: usize,
    pub last_error: Option<String>,
    /// Na fila ou buscando (não fica no arquivo).
    #[serde(skip_deserializing)]
    pub running: bool,
    #[serde(skip_deserializing)]
    #[ts(type = "number | null")]
    pub next_run: Option<u64>,
}

impl Default for Schedule {
    fn default() -> Self {
        Self {
            id: String::new(),
            query: String::new(),
            sites: Vec::new(),
            condition: ConditionFilter::Any,
            max_price: None,
            every: Every::default(),
            enabled: true,
            plan: None,
            last_run: None,
            last_total: 0,
            last_new: 0,
            last_error: None,
            running: false,
            next_run: None,
        }
    }
}

/// Resultado da última execução de uma busca automática.
#[derive(Clone, Debug, Default, Serialize, Deserialize, TS)]
#[serde(default)]
#[ts(export)]
pub struct ScheduleResults {
    #[ts(type = "number | null")]
    pub at: Option<u64>,
    pub listings: Vec<Listing>,
    /// Ids dos anúncios que não estavam na execução anterior.
    pub new: Vec<String>,
}

#[derive(Clone, Debug, Serialize, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
#[ts(export)]
pub enum ScheduleEvent {
    /// A busca automática mudou (começou, terminou, foi editada).
    Changed { schedule: Box<Schedule> },
    Removed { id: String },
    /// Uso da IA de uma execução (para os totais da interface).
    Usage { usage: Usage },
    /// Andamento de uma execução: lojas (`started`), mensagens (`log`) e fim
    /// de cada loja (`agent_finished`).
    Progress { id: String, event: Box<SearchEvent> },
}

/// Estado das buscas automáticas dentro do `Engine`.
pub(crate) struct State {
    dir: PathBuf,
    list: Mutex<Vec<Schedule>>,
    memos: Mutex<HashMap<String, Arc<Mutex<Memo>>>>,
    /// Busca automática → job (vazio enquanto espera a vez).
    running: Mutex<HashMap<String, String>>,
    tx: Mutex<Option<mpsc::UnboundedSender<ScheduleEvent>>>,
    /// Uma execução por vez: buscas automáticas não disputam CPU entre si.
    turn: tokio::sync::Mutex<()>,
    started: AtomicBool,
}

impl State {
    pub fn load(dir: &Path) -> Self {
        let list = std::fs::read(dir.join("schedules.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        Self {
            dir: dir.to_path_buf(),
            list: Mutex::new(list),
            memos: Default::default(),
            running: Default::default(),
            tx: Default::default(),
            turn: Default::default(),
            started: AtomicBool::new(false),
        }
    }

    /// Jobs das buscas automáticas em andamento.
    pub fn jobs(&self) -> HashSet<String> {
        self.running.lock().values().filter(|j| !j.is_empty()).cloned().collect()
    }

    fn auto_dir(&self) -> PathBuf {
        self.dir.join("auto")
    }

    fn save(&self) -> Result<()> {
        let data = serde_json::to_vec_pretty(&*self.list.lock())?;
        std::fs::create_dir_all(&self.dir)?;
        let tmp = self.dir.join("schedules.json.tmp");
        std::fs::write(&tmp, data)?;
        std::fs::rename(tmp, self.dir.join("schedules.json"))?;
        Ok(())
    }

    fn write(&self, name: &str, value: &impl Serialize) -> Result<()> {
        let dir = self.auto_dir();
        std::fs::create_dir_all(&dir)?;
        let tmp = dir.join(format!("{name}.tmp"));
        std::fs::write(&tmp, serde_json::to_vec(value)?)?;
        std::fs::rename(tmp, dir.join(name))?;
        Ok(())
    }

    fn memo(&self, id: &str) -> Arc<Mutex<Memo>> {
        self.memos
            .lock()
            .entry(id.to_string())
            .or_insert_with(|| {
                let m = std::fs::read(self.auto_dir().join(format!("{id}.memo.json"))).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
                Arc::new(Mutex::new(m))
            })
            .clone()
    }

    /// Apaga o plano, os vereditos e o resultado: a busca começa do zero.
    fn forget(&self, id: &str) {
        self.memos.lock().remove(id);
        let _ = std::fs::remove_file(self.auto_dir().join(format!("{id}.memo.json")));
        let _ = std::fs::remove_file(self.auto_dir().join(format!("{id}.json")));
    }

    fn emit(&self, ev: ScheduleEvent) {
        if let Some(tx) = self.tx.lock().as_ref() {
            let _ = tx.send(ev);
        }
    }
}

/// Horário de hoje (local) em segundos desde 1970.
fn today_at(now: u64, hour: u8, minute: u8) -> Option<u64> {
    let date = Local.timestamp_opt(now as i64, 0).single()?.date_naive();
    let t = date.and_hms_opt(hour.into(), minute.into(), 0)?;
    Some(Local.from_local_datetime(&t).earliest()?.timestamp() as u64)
}

/// Próxima execução. Uma execução perdida (app fechado) roda assim que possível.
pub fn next_run(s: &Schedule, now: u64) -> Option<u64> {
    if !s.enabled {
        return None;
    }
    match s.every {
        Every::Interval { minutes } => Some(s.last_run.map_or(now, |t| t + u64::from(minutes.max(MIN_MINUTES)) * 60)),
        Every::Daily { hour, minute } => {
            let today = today_at(now, hour, minute)?;
            if s.last_run.is_some_and(|t| t >= today) {
                // Já rodou hoje: amanhã no mesmo horário.
                today_at(today + 26 * 3600, hour, minute)
            } else if now < today {
                Some(today)
            } else {
                Some(now)
            }
        }
    }
}

impl Engine {
    pub fn schedules(&self) -> Vec<Schedule> {
        let now = unix_now();
        let running = self.auto.running.lock().clone();
        self.auto
            .list
            .lock()
            .iter()
            .cloned()
            .map(|mut s| {
                s.running = running.contains_key(&s.id);
                s.next_run = next_run(&s, now);
                s
            })
            .collect()
    }

    fn schedule(&self, id: &str) -> Option<Schedule> {
        self.schedules().into_iter().find(|s| s.id == id)
    }

    fn changed(&self, id: &str) {
        if let Some(s) = self.schedule(id) {
            self.auto.emit(ScheduleEvent::Changed { schedule: Box::new(s) });
        }
    }

    /// Cria (id vazio) ou altera uma busca automática. Mudar a frase, a
    /// condição ou o preço máximo apaga o plano e os vereditos guardados.
    pub fn save_schedule(self: &Arc<Self>, mut s: Schedule) -> Result<Schedule> {
        s.query = s.query.trim().to_string();
        if s.query.chars().count() < 2 {
            bail!("Descreva o produto que você quer buscar.");
        }
        if s.sites.is_empty() {
            bail!("Selecione pelo menos uma loja.");
        }
        match &mut s.every {
            Every::Interval { minutes } if *minutes < MIN_MINUTES => bail!("O intervalo mínimo é de {MIN_MINUTES} minutos."),
            Every::Daily { hour, minute } if *hour > 23 || *minute > 59 => bail!("Digite um horário válido."),
            _ => {}
        }
        let id = {
            let mut list = self.auto.list.lock();
            match list.iter_mut().find(|x| x.id == s.id && !s.id.is_empty()) {
                Some(old) => {
                    let reset = old.query != s.query || old.condition != s.condition || old.max_price != s.max_price;
                    if reset {
                        self.auto.forget(&old.id);
                        old.plan = None;
                        old.last_total = 0;
                        old.last_new = 0;
                    }
                    old.query = s.query;
                    old.sites = s.sites;
                    old.condition = s.condition;
                    old.max_price = s.max_price;
                    old.every = s.every;
                    old.enabled = s.enabled;
                    old.id.clone()
                }
                None => {
                    let fresh = Schedule {
                        id: uuid::Uuid::new_v4().simple().to_string()[..10].to_string(),
                        query: s.query,
                        sites: s.sites,
                        condition: s.condition,
                        max_price: s.max_price,
                        every: s.every,
                        enabled: s.enabled,
                        ..Default::default()
                    };
                    let id = fresh.id.clone();
                    list.push(fresh);
                    id
                }
            }
        };
        self.auto.save()?;
        self.changed(&id);
        self.schedule(&id).ok_or_else(|| anyhow!("busca automática desconhecida"))
    }

    pub fn delete_schedule(&self, id: &str) -> Result<()> {
        self.stop_schedule(id);
        self.auto.list.lock().retain(|s| s.id != id);
        self.auto.forget(id);
        self.auto.save()?;
        self.auto.emit(ScheduleEvent::Removed { id: id.to_string() });
        Ok(())
    }

    pub fn stop_schedule(&self, id: &str) {
        let job = self.auto.running.lock().get(id).cloned();
        if let Some(j) = job.filter(|j| !j.is_empty()) {
            self.cancel(&j);
        }
    }

    pub fn schedule_results(&self, id: &str) -> ScheduleResults {
        std::fs::read(self.auto.auto_dir().join(format!("{id}.json"))).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    /// Começa a conferir as buscas automáticas. Os avisos chegam em `tx`.
    pub fn start_scheduler(self: &Arc<Self>, tx: mpsc::UnboundedSender<ScheduleEvent>) {
        *self.auto.tx.lock() = Some(tx);
        if self.auto.started.swap(true, Ordering::SeqCst) {
            return;
        }
        let me = self.clone();
        self.rt.spawn(async move {
            // Deixa o app abrir antes da primeira busca.
            tokio::time::sleep(Duration::from_secs(10)).await;
            loop {
                let now = unix_now();
                for s in me.schedules() {
                    if !s.running && s.next_run.is_some_and(|n| n <= now) {
                        let _ = me.run_schedule(&s.id);
                    }
                }
                tokio::time::sleep(TICK).await;
            }
        });
    }

    /// Põe a busca automática na fila agora.
    pub fn run_schedule(self: &Arc<Self>, id: &str) -> Result<()> {
        if self.schedule(id).is_none() {
            bail!("busca automática desconhecida");
        }
        {
            let mut running = self.auto.running.lock();
            if running.contains_key(id) {
                return Ok(());
            }
            running.insert(id.to_string(), String::new());
        }
        self.changed(id);
        let me = self.clone();
        let id = id.to_string();
        self.rt.spawn(async move {
            let error = me.execute(&id).await.err().map(|e| format!("{e:#}"));
            me.auto.running.lock().remove(&id);
            if let Some(e) = &error {
                tracing::warn!("busca automática {id}: {e}");
            }
            {
                let mut list = me.auto.list.lock();
                if let Some(s) = list.iter_mut().find(|s| s.id == id) {
                    // Uma execução com erro também conta: sem isso, o Snipr tentaria de novo a cada 20 s.
                    if error.is_some() {
                        s.last_run = Some(unix_now());
                    }
                    s.last_error = error;
                }
            }
            let _ = me.auto.save();
            me.changed(&id);
        });
        Ok(())
    }

    async fn execute(self: &Arc<Self>, id: &str) -> Result<()> {
        let _turn = self.auto.turn.lock().await;
        let s = self.schedule(id).ok_or_else(|| anyhow!("busca automática removida"))?;
        let req = SearchRequest { query: s.query.clone(), condition: s.condition, sites: s.sites.clone(), max_price: s.max_price, plan: s.plan.clone() };
        let memo = self.auto.memo(id);
        let (tx, mut rx) = mpsc::unbounded_channel();
        let (done_tx, done_rx) = oneshot::channel();
        let job = self.launch(req, tx, Some(memo.clone()), Some(done_tx))?;
        if let Some(j) = self.auto.running.lock().get_mut(id) {
            *j = job;
        }
        self.changed(id);
        let (mut failed, mut total, mut first_error) = (0, 0, None);
        while let Some(ev) = rx.recv().await {
            match ev {
                SearchEvent::Usage { usage, .. } => self.auto.emit(ScheduleEvent::Usage { usage }),
                SearchEvent::AgentFinished { ok, ref summary, .. } => {
                    total += 1;
                    if !ok {
                        failed += 1;
                        first_error.get_or_insert(summary.clone());
                    }
                    self.auto.emit(ScheduleEvent::Progress { id: id.to_string(), event: Box::new(ev) });
                }
                SearchEvent::Started { .. } | SearchEvent::Log { .. } => {
                    self.auto.emit(ScheduleEvent::Progress { id: id.to_string(), event: Box::new(ev) })
                }
                SearchEvent::Finished { .. } => break,
                _ => {}
            }
        }
        let r = done_rx.await.map_err(|_| anyhow!("a busca parou"))?;
        // Parada pelo usuário: o resultado anterior continua valendo, e a
        // próxima execução segue o intervalo normal.
        if r.cancelled {
            if let Some(x) = self.auto.list.lock().iter_mut().find(|x| x.id == id) {
                x.last_run = Some(unix_now());
            }
            return Ok(());
        }

        let prev = self.schedule_results(id);
        let seen: HashSet<String> = prev.listings.iter().map(|l| dedupe_key(&l.url, &l.title)).collect();
        let new: Vec<String> =
            if prev.at.is_none() { Vec::new() } else { r.listings.iter().filter(|l| !seen.contains(&dedupe_key(&l.url, &l.title))).map(|l| l.id.clone()).collect() };
        let now = unix_now();
        self.auto.write(&format!("{id}.json"), &ScheduleResults { at: Some(now), listings: r.listings.clone(), new: new.clone() })?;
        self.auto.write(&format!("{id}.memo.json"), &*memo.lock())?;
        if self.settings().history_auto {
            let entry = history::Entry {
                at: now,
                query: &s.query,
                schedule: Some(id),
                sites: &s.sites,
                plan: r.plan.as_ref(),
                elapsed_ms: r.elapsed_ms,
                listings: &r.listings,
            };
            if let Err(e) = self.history.append(history::Kind::Auto, &entry) {
                tracing::warn!("não foi possível gravar o histórico: {e:#}");
            }
        }
        {
            let mut list = self.auto.list.lock();
            if let Some(x) = list.iter_mut().find(|x| x.id == id) {
                if x.plan.is_none() && x.query == s.query {
                    x.plan = r.plan;
                }
                x.last_run = Some(now);
                x.last_total = r.listings.len();
                x.last_new = new.len();
            }
        }
        if total > 0 && failed == total {
            bail!("{}", first_error.unwrap_or_default());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calcula_a_proxima_execucao() {
        let now = today_at(unix_now(), 12, 0).unwrap();
        let mut s = Schedule { every: Every::Interval { minutes: 30 }, ..Default::default() };
        assert_eq!(next_run(&s, now), Some(now), "nunca rodou: roda já");
        s.last_run = Some(now - 600);
        assert_eq!(next_run(&s, now), Some(now - 600 + 1800));
        s.every = Every::Daily { hour: 15, minute: 30 };
        assert_eq!(next_run(&s, now), Some(now + 3 * 3600 + 1800), "hoje mais tarde");
        s.every = Every::Daily { hour: 9, minute: 0 };
        s.last_run = Some(now - 86_400);
        assert_eq!(next_run(&s, now), Some(now), "o horário de hoje passou sem rodar");
        s.last_run = Some(now - 3600);
        let next = next_run(&s, now).unwrap();
        assert!(next > now + 20 * 3600 && next < now + 22 * 3600, "amanhã às 9h");
        s.enabled = false;
        assert_eq!(next_run(&s, now), None);
    }

    /// Roda uma busca automática duas vezes com os ajustes reais (usa a IA).
    /// A segunda execução reusa o plano e os vereditos:
    ///   SNIPR_TEST_SETTINGS=<settings.json> SNIPR_TEST_SITES=olx cargo test -p snipr-core --release automatica_reusa -- --ignored --nocapture
    #[tokio::test]
    #[ignore]
    async fn automatica_reusa_plano_e_vereditos() {
        let tmp = std::env::temp_dir().join("snipr-auto-test");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::copy(std::env::var("SNIPR_TEST_SETTINGS").unwrap(), tmp.join("settings.json")).unwrap();
        let paths = crate::config::Paths { config_file: tmp.join("settings.json"), profile_dir: tmp.join("profile"), work_dir: tmp.join("agents") };
        let engine = Engine::with_paths(paths).await.unwrap();
        let (tx, mut rx) = mpsc::unbounded_channel();
        engine.start_scheduler(tx);
        let sites: Vec<String> = std::env::var("SNIPR_TEST_SITES").unwrap_or_else(|_| "olx".into()).split(',').map(String::from).collect();
        let query = std::env::var("SNIPR_TEST_QUERY").unwrap_or_else(|_| "playstation 5".into());
        let s = engine.save_schedule(Schedule { query, sites, every: Every::Interval { minutes: 600 }, ..Default::default() }).unwrap();
        for round in 1..=2 {
            if round == 2 {
                engine.run_schedule(&s.id).unwrap();
            }
            let started = std::time::Instant::now();
            let (mut tokens, mut calls) = (0, 0);
            loop {
                match rx.recv().await.unwrap() {
                    ScheduleEvent::Usage { usage } => {
                        calls += 1;
                        tokens += usage.input_tokens + usage.output_tokens;
                    }
                    ScheduleEvent::Changed { schedule } if !schedule.running && schedule.last_run.is_some() => {
                        println!(
                            "execução {round}: {} resultados, {} novos, erro {:?}, {calls} chamadas à IA, {tokens} tokens, {:.0}s",
                            schedule.last_total,
                            schedule.last_new,
                            schedule.last_error,
                            started.elapsed().as_secs_f64()
                        );
                        assert!(schedule.plan.is_some(), "guarda o plano");
                        break;
                    }
                    _ => {}
                }
            }
            let r = engine.schedule_results(&s.id);
            for l in r.listings.iter().take(8) {
                println!("  R$ {:>8.2} cartão {:?} {:?}x {:?} {}", l.price, l.card_price, l.installments, l.offers, l.title.chars().take(60).collect::<String>());
            }
        }
        engine.shutdown().await;
    }

    #[test]
    fn evento_vira_json_com_tipo() {
        let v = serde_json::to_value(ScheduleEvent::Removed { id: "x".into() }).unwrap();
        assert_eq!(v["type"], "removed");
        let e: Every = serde_json::from_value(serde_json::json!({"kind": "daily", "hour": 9, "minute": 30})).unwrap();
        assert_eq!(e, Every::Daily { hour: 9, minute: 30 });
    }
}
