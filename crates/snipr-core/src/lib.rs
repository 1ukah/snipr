//! Núcleo do Snipr.
//!
//! Frontends (app desktop, bots de Telegram/Discord/WhatsApp, API de um SaaS)
//! usam só o `Engine`: `start_search(SearchRequest, sink)` e consomem
//! `SearchEvent`s. Nada aqui depende de interface gráfica.
//!
//! Cada loja é lida na etapa mais leve que funciona (ver `sites::Stage`):
//! HTTP (A, B, C) ou Chrome (D). A IA filtra os anúncios em chamadas curtas;
//! o agent com ferramentas fica só para lojas sem leitura automática.

mod agent;
pub mod cli;
pub mod config;
mod engine;
mod fetch;
mod history;
mod mcp;
mod models;
mod offers;
mod parse;
mod prompts;
pub mod schedule;
mod search;
pub mod secrets;
pub mod sites;
mod stage;
mod tools;
pub mod types;

pub use cli::CliStatus;
pub use config::{AiSettings, AuthMode, Provider, SessionState, Settings, Theme};
pub use engine::{Engine, EventSink};
pub use schedule::{Every, Schedule, ScheduleEvent, ScheduleResults};
pub use sites::{Api, Site, Stage, StageInfo, Suggestion};
pub use snipr_browser::BrowserMode;
pub use types::*;
