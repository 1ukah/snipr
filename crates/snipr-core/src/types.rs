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

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum AgentMode {
    /// Um agente percorre todos os sites, em sequência. Mais barato.
    Single,
    /// Um agente por site, em paralelo (limitado por `max_parallel`). Mais rápido.
    #[default]
    Multi,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default, TS)]
#[serde(default)]
#[ts(export)]
pub struct SearchPlan {
    pub product: String,
    pub alternatives: Vec<String>,
    pub requirements: Vec<String>,
    pub exclude: Vec<String>,
    pub condition: Option<ConditionFilter>,
    pub max_price: Option<f64>,
    pub search_terms: Vec<String>,
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
    pub mode: Option<AgentMode>,
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
    Finished { job: String, total: usize, #[ts(type = "number")] elapsed_ms: u128, cancelled: bool },
}

#[derive(Clone, Debug, Serialize, TS)]
#[ts(export)]
pub struct AgentInfo {
    pub id: String,
    pub label: String,
    pub sites: Vec<String>,
}
