//! Tipos públicos do Snipr. Qualquer "frontend" (GUI, bot de Telegram,
//! Discord, API HTTP de um futuro SaaS) fala com o core só por meio deles:
//! manda um `SearchRequest` e consome um fluxo de `SearchEvent`.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum Condition {
    New,
    Used,
    #[default]
    Unknown,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ConditionFilter {
    #[default]
    Any,
    New,
    Used,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum AgentMode {
    /// Um agente percorre todos os sites, em sequência. Mais barato.
    #[default]
    Single,
    /// Um agente por site, em paralelo (limitado por `max_parallel`). Mais rápido.
    Multi,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SearchRequest {
    pub query: String,
    #[serde(default)]
    pub condition: ConditionFilter,
    /// IDs de sites. Vazio = sites habilitados nos ajustes.
    #[serde(default)]
    pub sites: Vec<String>,
    #[serde(default)]
    pub mode: AgentMode,
    #[serde(default)]
    pub max_price: Option<f64>,
    #[serde(default)]
    pub notes: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
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
    pub seller: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shipping: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SearchEvent {
    Started { job: String, agents: Vec<AgentInfo> },
    Log { job: String, agent: String, message: String },
    Listing { job: String, agent: String, listing: Box<Listing> },
    NeedsHuman { job: String, agent: String, site: Option<String>, reason: String },
    HumanResolved { job: String, agent: String },
    AgentFinished { job: String, agent: String, ok: bool, summary: String },
    Finished { job: String, total: usize, elapsed_ms: u128, cancelled: bool },
}

#[derive(Clone, Debug, Serialize)]
pub struct AgentInfo {
    pub id: String,
    pub label: String,
    pub sites: Vec<String>,
}
