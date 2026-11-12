//! Tipos públicos do Snipr. Qualquer "frontend" (GUI, bot de Telegram,
//! Discord, API HTTP de um futuro SaaS) fala com o core só por meio deles:
//! manda um `SearchRequest` e consome um fluxo de `SearchEvent`.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum Condition {
    New,
    Used,
    #[default]
    Unknown,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ConditionFilter {
    #[default]
    Any,
    New,
    Used,
}

impl ConditionFilter {
    pub fn from_conditions(c: &[Condition]) -> Self {
        let new = c.contains(&Condition::New);
        let used = c.contains(&Condition::Used);
        match (new, used) {
            (true, false) => Self::New,
            (false, true) => Self::Used,
            _ => Self::Any,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default, TS)]
#[serde(default)]
#[ts(export)]
pub struct SearchPlan {
    pub product: String,
    pub alternatives: Vec<String>,
    pub requirements: Vec<String>,
    /// O que o usuário prefere, sem exigir ("de preferência X3D").
    pub preferences: Vec<String>,
    pub exclude: Vec<String>,
    pub condition: Option<ConditionFilter>,
    pub max_price: Option<f64>,
    pub search_terms: Vec<String>,
    /// Termos genéricos ("pc gamer") de anúncios que só citam as
    /// especificações na descrição. Com eles, a busca é profunda: anúncios
    /// genéricos têm a página lida.
    pub broad_terms: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SearchRequest {
    pub query: String,
    #[serde(default)]
    pub condition: ConditionFilter,
    /// IDs de sites. Vazio = sites habilitados nos ajustes.
    #[serde(default)]
    pub sites: Vec<String>,
    #[serde(default)]
    pub max_price: Option<f64>,
    #[serde(default)]
    pub plan: Option<SearchPlan>,
}

#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Listing {
    pub id: String,
    pub site_id: String,
    pub site_name: String,
    pub title: String,
    pub price: f64,
    pub currency: String,
    pub condition: Condition,
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub seller: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub location: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub shipping: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub notes: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub original_price: Option<f64>,
    #[serde(default)]
    pub international: bool,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum HumanKind {
    Login,
    Expired,
    Challenge,
    Other,
}

#[derive(Clone, Debug, Serialize, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
#[ts(export)]
pub enum SearchEvent {
    Started { job: String, agents: Vec<AgentInfo> },
    Planned { job: String, plan: SearchPlan },
    Log { job: String, agent: String, message: String },
    Listing { job: String, agent: String, listing: Box<Listing> },
    NeedsHuman { job: String, agent: String, site: Option<String>, kind: HumanKind, reason: String },
    HumanResolved { job: String, agent: String },
    AgentFinished { job: String, agent: String, ok: bool, summary: String },
    Usage { job: String, agent: Option<String>, usage: Usage },
    Finished { job: String, total: usize, #[ts(type = "number")] elapsed_ms: u128, cancelled: bool },
}

/// Uma loja em andamento numa busca (o nome segue "agent" por compatibilidade
/// com os eventos já consumidos pelos frontends).
#[derive(Clone, Debug, Serialize, TS)]
#[ts(export)]
pub struct AgentInfo {
    pub id: String,
    pub label: String,
    pub sites: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize, TS)]
#[ts(export)]
pub struct Usage {
    #[ts(type = "number")]
    pub input_tokens: u64,
    #[ts(type = "number")]
    pub output_tokens: u64,
    pub cost_usd: Option<f64>,
    pub model: Option<String>,
}

/// Navegador compatível encontrado no sistema.
#[derive(Clone, Debug, Serialize, TS)]
#[ts(export)]
pub struct DetectedBrowser {
    pub name: String,
    pub path: String,
    pub version: Option<String>,
}

#[derive(Clone, Debug, Serialize, TS)]
#[ts(export)]
pub struct BrowserStatus {
    pub browsers: Vec<DetectedBrowser>,
    /// Caminho que o Snipr vai usar (o dos ajustes ou o primeiro encontrado).
    pub selected: Option<String>,
    /// O caminho dos ajustes não existe.
    pub custom_missing: bool,
}

impl Usage {
    pub fn from_turn(t: &snipr_llm::Turn) -> Self {
        Self {
            input_tokens: t.input_tokens,
            output_tokens: t.output_tokens,
            cost_usd: t.cost_usd,
            model: Some(t.model.clone()).filter(|m| !m.is_empty()),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.input_tokens == 0 && self.output_tokens == 0 && self.cost_usd.is_none()
    }
}
