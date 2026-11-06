//! Núcleo do Snipr.
//!
//! Frontends (app desktop, bots de Telegram/Discord/WhatsApp, API de um SaaS)
//! usam só o `Engine`: `start_search(SearchRequest, sink)` e consomem
//! `SearchEvent`s. Nada aqui depende de interface gráfica.

mod agent;
pub mod cli;
pub mod config;
mod engine;
mod mcp;
mod prompts;
pub mod secrets;
pub mod sites;
mod tools;
pub mod types;

pub use cli::CliStatus;
pub use config::{AiSettings, AuthMode, Provider, SessionState, Settings, Theme};
pub use engine::{Engine, EventSink};
pub use sites::{Site, Suggestion};
pub use snipr_browser::BrowserMode;
pub use types::*;
