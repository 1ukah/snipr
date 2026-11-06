//! Camada de navegador do Snipr.
//!
//! - Chrome real (não headless-shell), perfil persistente
//! - CDP próprio: sem `Runtime.enable`, scripts em isolated world
//! - Uma conexão, várias abas (cada agente = uma janela no mesmo processo)
//! - Mouse/teclado com eventos confiáveis (`isTrusted`) e cadência humana
//! - Resolução automática de desafios Cloudflare quando possível

mod browser;
mod cdp;
mod challenge;
mod human;
mod js;
mod page;

pub use browser::{find_chrome, Browser, BrowserMode, LaunchOptions};
pub use cdp::{Cdp, CdpError, CdpEvent};
pub use challenge::ChallengeOutcome;
pub use page::{NavInfo, Page};
