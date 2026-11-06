//! Núcleo do Snipr.
//!
//! Frontends (app desktop, bots de Telegram/Discord/WhatsApp, API de um SaaS)
//! usam só o `Engine`: `start_search(SearchRequest, sink)` e consomem
//! `SearchEvent`s. Nada aqui depende de interface gráfica.

mod agent;
mod claude_code;
pub mod config;
mod engine;
mod mcp;
mod prompts;
pub mod secrets;
pub mod sites;
mod tools;
pub mod types;

pub use config::{ProviderKind, Settings};
pub use engine::{Engine, EventSink};
pub use sites::Site;
pub use snipr_browser::BrowserMode;
pub use types::*;
