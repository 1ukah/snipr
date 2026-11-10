//! Camada de navegador do Snipr.
//!
//! - Chrome real (não headless-shell), perfil persistente
//! - CDP próprio: sem `Runtime.enable`, scripts em isolated world
//! - Uma conexão, várias abas (cada agente = uma janela no mesmo processo),
//!   reusadas e estacionadas em about:blank quando ociosas
//! - Mouse/teclado com eventos confiáveis (`isTrusted`) e cadência humana
//! - Resolução automática de desafios Cloudflare quando possível

mod browser;
mod cdp;
mod challenge;
mod human;
mod js;
#[cfg(target_os = "macos")]
mod macos;
mod page;

pub use browser::{detect_browsers, find_chrome, Browser, BrowserInfo, BrowserMode, LaunchOptions, Lease};
pub use cdp::{Cdp, CdpError, CdpEvent};
pub use challenge::ChallengeOutcome;
pub use page::{Card, NavInfo, Page};
