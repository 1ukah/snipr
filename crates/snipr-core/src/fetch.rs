//! Leitura de páginas sem navegador (etapas A, B e C).
//!
//! - A: cliente HTTP comum.
//! - B: cliente com fingerprint TLS/HTTP2 de Chrome; passa por proteções que
//!   recusam clientes comuns pelo handshake.
//! - C: como B, com os cookies que o Chrome obteve ao passar pela proteção
//!   anti-bot da loja (ex.: Akamai). Os cookies ficam só na memória.

use std::{collections::HashMap, time::Duration};

use anyhow::{bail, Result};
use parking_lot::Mutex;

use crate::sites::Stage;

/// User-Agent da etapa A. A versão importa pouco: o que as lojas checam é o
/// handshake, e é por isso que a etapa B existe.
const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/142.0.0.0 Safari/537.36";
const ACCEPT: &str = "text/html,application/xhtml+xml,application/xml;q=0.9,application/json;q=0.8,*/*;q=0.7";
const LANG: &str = "pt-BR,pt;q=0.9,en-US;q=0.8,en;q=0.7";
/// Cookies velhos demais costumam ter expirado na proteção anti-bot.
const COOKIE_TTL: Duration = Duration::from_secs(20 * 60);

pub struct Fetched {
    pub status: u16,
    pub url: String,
    pub body: String,
}

pub struct Fetcher {
    plain: reqwest::Client,
    mimic: wreq::Client,
    cookies: Mutex<HashMap<String, (String, std::time::Instant)>>,
}

impl Fetcher {
    pub fn new() -> Result<Self> {
        let mut h = reqwest::header::HeaderMap::new();
        h.insert("user-agent", UA.parse()?);
        h.insert("accept", ACCEPT.parse()?);
        h.insert("accept-language", LANG.parse()?);
        let plain = reqwest::Client::builder()
            .default_headers(h)
            .timeout(Duration::from_secs(25))
            .connect_timeout(Duration::from_secs(10))
            .build()?;
        let platform = if cfg!(windows) {
            wreq_util::Platform::Windows
        } else if cfg!(target_os = "macos") {
            wreq_util::Platform::MacOS
        } else {
            wreq_util::Platform::Linux
        };
        let emulation = wreq_util::Emulation::builder().profile(wreq_util::Profile::Chrome149).platform(platform).build();
        let mimic = wreq::Client::builder()
            .emulation(emulation)
            .redirect(wreq::redirect::Policy::limited(10))
            .timeout(Duration::from_secs(25))
            .connect_timeout(Duration::from_secs(10))
            .build()?;
        Ok(Self { plain, mimic, cookies: Mutex::new(HashMap::new()) })
    }

    /// GET na etapa pedida. A etapa C usa o cookie informado.
    pub async fn get(&self, stage: Stage, url: &str, cookie: Option<&str>) -> Result<Fetched> {
        match stage {
            Stage::A => {
                let r = self.plain.get(url).send().await?;
                let status = r.status().as_u16();
                let url = r.url().to_string();
                Ok(Fetched { status, url, body: r.text().await? })
            }
            Stage::B | Stage::C => {
                let mut req = self.mimic.get(url).header("accept-language", LANG);
                if let Some(c) = cookie.filter(|c| !c.is_empty()) {
                    req = req.header("cookie", c);
                }
                let r = req.send().await?;
                let status = r.status().as_u16();
                let url = r.uri().to_string();
                Ok(Fetched { status, url, body: r.text().await? })
            }
            Stage::D => bail!("a etapa D usa o Chrome"),
        }
    }

    pub fn cookie(&self, site: &str) -> Option<String> {
        let mut map = self.cookies.lock();
        match map.get(site) {
            Some((c, at)) if at.elapsed() < COOKIE_TTL => Some(c.clone()),
            Some(_) => {
                map.remove(site);
                None
            }
            None => None,
        }
    }

    pub fn set_cookie(&self, site: &str, cookie: String) {
        self.cookies.lock().insert(site.to_string(), (cookie, std::time::Instant::now()));
    }

    pub fn forget_cookie(&self, site: &str) {
        self.cookies.lock().remove(site);
    }
}

/// Motivo de bloqueio (status, desafio anti-bot, login), ou `None` se a
/// página parece uma página normal.
pub fn blocked(f: &Fetched) -> Option<String> {
    if !(200..300).contains(&f.status) {
        return Some(format!("HTTP {}", f.status));
    }
    // Páginas de desafio são curtas; uma página de resultados tem centenas de KB.
    if f.body.len() > 60_000 {
        return None;
    }
    let low = f.body.to_lowercase();
    const MARKERS: &[&str] = &[
        "just a moment",
        "cf-chl",
        "challenge-platform",
        "px-captcha",
        "awswafcookiedomainlist",
        "_incapsula_",
        "captcha",
        "não é possível acessar",
        "access denied",
        "acesse sua conta",
        "requires javascript",
        "enable javascript",
        "browser-assessment",
        "powered and protected by",
    ];
    MARKERS.iter().find(|m| low.contains(*m)).map(|m| format!("proteção anti-bot ({m})"))
}

/// A página diz que a busca não achou nada (resultado legítimo, não bloqueio).
pub fn says_no_results(body: &str) -> bool {
    let low = body.to_lowercase();
    ["nenhum resultado", "não encontramos", "nao encontramos", "nenhum produto encontrado", "0 resultados", "sem resultados", "não foram encontrados"]
        .iter()
        .any(|m| low.contains(m))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(status: u16, body: &str) -> Fetched {
        Fetched { status, url: "https://x".into(), body: body.into() }
    }

    #[test]
    fn reconhece_bloqueios() {
        assert!(blocked(&page(403, "")).is_some());
        assert!(blocked(&page(200, "<title>Just a moment...</title>")).is_some());
        assert!(blocked(&page(200, "Olá! Para continuar, acesse sua conta")).is_some());
        assert!(blocked(&page(200, "<html>resultados R$ 10</html>")).is_none());
        let big = format!("<html>{}captcha</html>", "x".repeat(70_000));
        assert!(blocked(&page(200, &big)).is_none(), "páginas grandes não são desafio");
    }
}
