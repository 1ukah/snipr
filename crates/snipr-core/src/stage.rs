//! Verificação de loja: acha a URL de busca e a etapa mais leve que lê os
//! resultados corretamente.
//!
//! O Chrome lê uma busca de referência. Cada etapa HTTP (A, B, C) só é aceita
//! se o que ela lê cobre os anúncios da referência. Assim a escolha vale para
//! qualquer loja, sem regras por site.

use std::sync::Arc;

use anyhow::Result;
use snipr_browser::Card;

use crate::{
    engine::{unix_now, Engine},
    fetch, parse,
    sites::{Api, Site, Stage, StageInfo},
};

/// Fração mínima dos anúncios da referência que uma etapa HTTP precisa ler.
const MIN_COVERAGE: f64 = 0.7;
/// Fração mínima de cards que citam a busca para a página contar como resultado.
const MIN_RELEVANCE: f64 = 0.4;
/// Navegações no Chrome para achar a URL de busca.
const MAX_TRIES: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Platform {
    Vtex,
    Shopify,
    Nuvemshop,
    WooCommerce,
    Magento,
    Tray,
    Vnda,
    LojaIntegrada,
    Wake,
    Other,
}

fn platform_of(html: &str) -> Platform {
    let l = html.to_lowercase();
    let has = |w: &[&str]| w.iter().any(|x| l.contains(x));
    if has(&["vteximg", "vtexassets", "vtex.render", "vtexcommercestable"]) {
        Platform::Vtex
    } else if has(&["cdn.shopify.com", "shopify.theme"]) {
        Platform::Shopify
    } else if has(&["nuvemshop", "tiendanube"]) {
        Platform::Nuvemshop
    } else if has(&["woocommerce"]) {
        Platform::WooCommerce
    } else if has(&["mage/cookies", "magento", "/static/version"]) {
        Platform::Magento
    } else if has(&["tcdn.com.br", "tray.com.br"]) {
        Platform::Tray
    } else if has(&["vnda.com.br", "cdn.vnda"]) {
        Platform::Vnda
    } else if has(&["lojaintegrada"]) {
        Platform::LojaIntegrada
    } else if has(&["fbits", "wake.tech"]) {
        Platform::Wake
    } else {
        Platform::Other
    }
}

/// Caminhos de busca conhecidos de cada plataforma, depois os genéricos.
fn patterns(p: Platform) -> Vec<&'static str> {
    let own: &[&str] = match p {
        Platform::Vtex => &["/{q}?_q={q}&map=ft", "/s?q={q}"],
        Platform::Shopify => &["/search?q={q}&type=product"],
        Platform::Nuvemshop => &["/search/?q={q}"],
        Platform::WooCommerce => &["/?s={q}&post_type=product"],
        Platform::Magento => &["/catalogsearch/result/?q={q}"],
        Platform::Tray => &["/loja/busca.php?palavra_busca={q}"],
        Platform::Vnda => &["/busca?q={q}"],
        Platform::LojaIntegrada => &["/buscar?q={q}"],
        Platform::Wake => &["/busca?busca={q}"],
        Platform::Other => &[],
    };
    let mut v = own.to_vec();
    v.extend(["/search?q={q}", "/busca?q={q}", "/s?q={q}", "/busca/{q}", "/pesquisa?q={q}"]);
    v
}

/// Foca o campo de busca mais provável da página (para digitar como uma pessoa).
const FOCUS_SEARCH: &str = r#"(() => {
  const vis = (el) => { const r = el.getBoundingClientRect(); const s = getComputedStyle(el); return r.width > 20 && r.height > 8 && s.visibility !== 'hidden' && s.display !== 'none'; };
  const all = [...document.querySelectorAll('input:not([type=hidden]):not([type=checkbox]):not([type=radio]):not([type=password]):not([type=email])')];
  const score = (i) => (i.type === 'search' ? 5 : 0)
    + (/busca|search|pesquis|procur|termo|keyword|^q$/i.test([i.name, i.id, i.placeholder, i.getAttribute('aria-label')].join(' ')) ? 5 : 0)
    + (vis(i) ? 3 : 0);
  const best = all.sort((a, b) => score(b) - score(a))[0];
  if (!best || score(best) < 8) return false;
  best.scrollIntoView({ block: 'center' });
  best.focus();
  best.value = '';
  return true;
})()"#;

/// Palavra que a loja vende, tirada dos cards da home (para testar a busca).
/// Títulos de produto costumam começar pelo tipo ("Notebook…", "Cadeira…"):
/// a primeira palavra mais comum é um termo de busca melhor que adjetivos
/// frequentes como "gamer", que algumas lojas levam a páginas de conteúdo.
fn probe_word(cards: &[Card]) -> Option<String> {
    const NOISE: &[&str] = &[
        "frete", "gratis", "oferta", "desconto", "promocao", "compre", "novo", "nova", "preto", "branco", "kit", "cor", "produto", "produtos",
    ];
    let ok = |w: &String| w.len() >= 4 && w.chars().all(|ch| ch.is_alphabetic()) && !NOISE.contains(&w.as_str());
    let top = |words: Vec<String>| {
        let mut count: std::collections::HashMap<String, usize> = Default::default();
        for w in words.into_iter().filter(ok) {
            *count.entry(w).or_default() += 1;
        }
        count.into_iter().filter(|(_, n)| *n >= 2).max_by_key(|(w, n)| (*n, std::cmp::Reverse(w.len()))).map(|(w, _)| w)
    };
    top(cards.iter().filter_map(|c| parse::tokens(&c.title).into_iter().next()).collect())
        .or_else(|| top(cards.iter().flat_map(|c| parse::tokens(&c.title)).collect()))
}

fn with_template(site: &Site, template: &str) -> Site {
    Site { search_url: template.to_string(), ..site.clone() }
}

/// A página mostra resultados da palavra. Só o título conta: o texto extra
/// repete botões como "Adicionar ao carrinho" em todo card.
fn good(cards: &[Card], word: &str) -> bool {
    let words = parse::tokens(word);
    let hits = cards.iter().filter(|c| words.iter().any(|w| parse::fold(&c.title).contains(w.as_str()))).count();
    cards.len() >= 3 && hits as f64 >= cards.len() as f64 * MIN_RELEVANCE
}

/// Palavra mais frequente no título, descrição, headings e menus da página
/// (quando a home não mostra cards).
fn title_word(html: &str, site: &Site) -> Option<String> {
    const NOISE: &[&str] = &[
        "loja", "site", "oficial", "compre", "comprar", "melhor", "melhore", "preco", "precos", "online", "brasil", "entrega", "qualidade",
        "produto", "produtos", "home", "inicio", "conta", "carrinho", "login", "entrar", "oferta", "ofertas", "black", "friday", "promocao",
        "promocoe", "todos", "todas", "frete", "gratis", "desconto", "novidade", "novidades", "cupom", "atendimento",
    ];
    let doc = scraper::Html::parse_document(html);
    let sel = |s: &str| scraper::Selector::parse(s).expect("seletor");
    let mut text = String::new();
    for s in doc.select(&sel("title, h1, h2, h3, nav a, header a")) {
        text.push_str(&s.text().collect::<String>());
        text.push(' ');
    }
    for s in doc.select(&sel(r#"meta[name="description"], meta[property="og:title"], meta[property="og:description"]"#)) {
        text.push_str(s.value().attr("content").unwrap_or_default());
        text.push(' ');
    }
    let own = parse::tokens(&format!("{} {}", site.name, site.domain));
    let mut count: std::collections::HashMap<String, usize> = Default::default();
    for w in parse::tokens(&text) {
        if w.len() >= 4 && w.chars().all(|c| c.is_alphabetic()) && !NOISE.contains(&w.as_str()) && !own.contains(&w) {
            *count.entry(w).or_default() += 1;
        }
    }
    count.into_iter().max_by_key(|(w, n)| (*n, std::cmp::Reverse(w.len()))).map(|(w, _)| w)
}

impl Engine {
    /// Acha a URL de busca e a etapa mais leve da loja.
    pub(crate) async fn detect_stage(self: &Arc<Self>, site: &Site, form_search: &str) -> Result<StageInfo> {
        self.hold();
        // Uma página que não termina de carregar não pode prender a verificação.
        let r = match tokio::time::timeout(std::time::Duration::from_secs(180), self.detect_stage_inner(site, form_search)).await {
            Ok(r) => r,
            Err(_) => Err(anyhow::anyhow!("a verificação da loja demorou demais")),
        };
        self.release();
        if let Ok(i) = &r {
            tracing::info!(site = %site.id, stage = ?i.stage, api = ?i.api, url = %i.search_url, json = %i.json_url, "loja verificada");
        }
        r
    }

    /// Lê a busca por HTTP e mede quanto da referência ela cobre. Se o HTML
    /// não basta, tenta aprender o link dos produtos do JSON embutido.
    async fn http_coverage(&self, stage: Stage, url: &str, cookie: Option<&str>, reference: &[Card]) -> Option<(f64, Option<String>)> {
        let f = self.fetcher.get(stage, url, cookie).await.ok()?;
        if let Some(why) = fetch::blocked(&f) {
            tracing::debug!(?stage, %why, "etapa bloqueada");
            return None;
        }
        let got = parse::cards_from_html(&f.body, &f.url, 80, None);
        let mut best = (parse::coverage(reference, &got), None);
        if best.0 < MIN_COVERAGE {
            if let Some(t) = parse::learn_json_url(&f.body, reference) {
                let cov = parse::coverage(reference, &parse::cards_from_html(&f.body, &f.url, 80, Some(&t)));
                if cov > best.0 {
                    best = (cov, Some(t));
                }
            }
        }
        tracing::debug!(?stage, got = got.len(), reference = reference.len(), cov = best.0, json = ?best.1, "etapa testada");
        Some(best)
    }

    async fn detect_stage_inner(self: &Arc<Self>, site: &Site, form_search: &str) -> Result<StageInfo> {
        let home = site.home();
        let mut home_html = None;
        for stage in [Stage::A, Stage::B] {
            if let Ok(f) = self.fetcher.get(stage, &home, None).await {
                if fetch::blocked(&f).is_none() {
                    home_html = Some(f);
                    break;
                }
            }
        }
        let platform = home_html.as_ref().map(|f| platform_of(&f.body)).unwrap_or(Platform::Other);
        let origin = site.origin();
        let info = |stage, api, url: &str, json: Option<String>| StageInfo {
            stage,
            api,
            search_url: url.to_string(),
            json_url: json.unwrap_or_default(),
            checked_at: unix_now(),
        };
        let home_word = home_html
            .as_ref()
            .and_then(|f| probe_word(&parse::cards_from_html(&f.body, &f.url, 60, None)).or_else(|| title_word(&f.body, site)));

        let browser = match self.browser().await {
            Ok(b) => b,
            // Sem navegador: só dá para testar as etapas HTTP pela relevância.
            Err(e) => {
                tracing::warn!("verificação sem navegador: {e:#}");
                let word = home_word.unwrap_or_else(|| "produto".into());
                let mut templates = Vec::new();
                if !site.search_url.is_empty() {
                    templates.push(site.search_url.clone());
                }
                templates.extend(patterns(platform).into_iter().map(|p| format!("{origin}{p}")));
                for t in templates {
                    let url = with_template(site, &t).build_search_url(&word);
                    for stage in [Stage::A, Stage::B] {
                        let Ok(f) = self.fetcher.get(stage, &url, None).await else { continue };
                        if fetch::blocked(&f).is_none() && good(&parse::cards_from_html(&f.body, &f.url, 60, None), &word) {
                            return Ok(info(stage, None, &t, None));
                        }
                    }
                }
                return Ok(info(Stage::D, None, &site.search_url, None));
            }
        };

        let lease = browser.lease().await?;
        lease.goto(&home).await?;
        let rendered = lease.eval("document.documentElement.outerHTML").await.ok().and_then(|v| v.as_str().map(String::from)).unwrap_or_default();
        let word = probe_word(&lease.extract_cards(40).await.unwrap_or_default())
            .or(home_word)
            .or_else(|| title_word(&rendered, site))
            .unwrap_or_else(|| "produto".into());

        // URL de busca: a conhecida; senão formulário, busca digitada e padrões.
        let mut templates: Vec<String> = Vec::new();
        if !site.search_url.is_empty() {
            templates.push(site.search_url.clone());
        }
        // Lojas adicionadas pelo usuário podem ter uma URL salva errada: procura de novo.
        if site.search_url.is_empty() || site.custom {
            if !form_search.is_empty() {
                templates.push(form_search.to_string());
            }
            if lease.eval(FOCUS_SEARCH).await.ok().and_then(|v| v.as_bool()) == Some(true) {
                let before = lease.url().await.unwrap_or_default();
                if lease.type_text(&word).await.is_ok() && lease.press_enter().await.is_ok() {
                    let href = lease.url().await.unwrap_or_default();
                    if href != before {
                        if let Some(i) = href.to_lowercase().find(&word) {
                            templates.push(format!("{}{{q}}{}", &href[..i], &href[i + word.len()..]));
                        }
                    }
                }
            }
            templates.extend(patterns(platform).into_iter().map(|p| format!("{origin}{p}")));
        }
        // O fragmento (#…) não vai para o servidor: uma busca só no fragmento
        // devolveria a home por HTTP.
        templates.retain(|t| t.find('#').is_none_or(|h| t.find("{q").is_some_and(|q| q < h)));
        templates.dedup();

        tracing::debug!(site = %site.id, ?platform, %word, ?templates, "procurando a busca");
        // Cada URL de busca válida no Chrome é testada por HTTP. A mesma loja
        // pode ter uma busca renderizada no navegador e outra que vem pronta
        // no HTML (ex.: VTEX): vale a primeira que dá para ler sem o Chrome.
        let words = parse::tokens(&word);
        let mut found: Option<(String, Vec<Card>)> = None;
        for t in templates.iter().take(MAX_TRIES) {
            let url = with_template(site, t).build_search_url(&word);
            // Página 404 que mostra "produtos relacionados" não é busca.
            if self.fetcher.get(Stage::A, &url, None).await.is_ok_and(|f| matches!(f.status, 404 | 410)) {
                tracing::debug!(%url, "busca inexistente (404)");
                continue;
            }
            if lease.goto(&url).await.is_err() {
                continue;
            }
            let cards = lease.extract_cards(40).await.unwrap_or_default();
            tracing::debug!(%url, cards = cards.len(), relevance = parse::relevance(&cards, &word), "busca testada");
            if !good(&cards, &word) {
                continue;
            }
            // Só os cards que citam a busca: anúncios patrocinados mudam a cada visita.
            let reference: Vec<Card> = cards.into_iter().filter(|c| words.iter().any(|w| parse::fold(&c.title).contains(w.as_str()))).collect();
            for stage in [Stage::A, Stage::B] {
                if let Some((cov, json)) = self.http_coverage(stage, &url, None, &reference).await {
                    if cov >= MIN_COVERAGE {
                        return Ok(info(stage, None, t, json));
                    }
                }
            }
            if found.is_none() {
                found = Some((t.clone(), reference));
            }
        }

        let Some((template, reference)) = found else {
            if platform == Platform::Vtex {
                if let Some(s) = self.vtex_stage(&origin, &word).await {
                    return Ok(info(s, Some(Api::Vtex), &format!("{origin}/{{q}}?_q={{q}}&map=ft"), None));
                }
            }
            // Nenhuma busca legível: o agent procura o campo de busca na hora.
            return Ok(info(Stage::D, None, "", None));
        };
        let url = with_template(site, &template).build_search_url(&word);
        if platform == Platform::Vtex {
            if let Some(s) = self.vtex_stage(&origin, &word).await {
                return Ok(info(s, Some(Api::Vtex), &template, None));
            }
        }
        // Etapa C: os cookies que o Chrome acabou de receber passam pela proteção?
        let cookie = lease.cookie_header(&[&url, &home]).await.unwrap_or_default();
        if !cookie.is_empty() {
            if let Some((cov, json)) = self.http_coverage(Stage::C, &url, Some(&cookie), &reference).await {
                if cov >= MIN_COVERAGE {
                    self.fetcher.set_cookie(&site.id, cookie);
                    return Ok(info(Stage::C, None, &template, json));
                }
            }
        }
        Ok(info(Stage::D, None, &template, None))
    }

    /// Catálogo VTEX: JSON público, quase sempre legível por HTTP.
    async fn vtex_stage(&self, origin: &str, word: &str) -> Option<Stage> {
        let url = format!("{origin}/api/catalog_system/pub/products/search?ft={}&_from=0&_to=39", urlencoding::encode(word));
        for stage in [Stage::A, Stage::B] {
            let Ok(f) = self.fetcher.get(stage, &url, None).await else { continue };
            if fetch::blocked(&f).is_none() && good(&parse::cards_from_vtex(&f.body, 40), word) {
                return Some(stage);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reconhece_plataformas() {
        assert_eq!(platform_of(r#"<link href="https://lojax.vteximg.com.br/a.css">"#), Platform::Vtex);
        assert_eq!(platform_of(r#"<script src="//cdn.shopify.com/s/x.js">"#), Platform::Shopify);
        assert_eq!(platform_of("<html>nuvemshop</html>"), Platform::Nuvemshop);
        assert_eq!(platform_of("<html>nada</html>"), Platform::Other);
        assert!(patterns(Platform::Magento)[0].starts_with("/catalogsearch"));
    }

    /// Verifica lojas reais (Chrome + internet, sem IA):
    ///   SNIPR_TEST_PROFILE=<cópia do perfil> SNIPR_TEST_SITES=kabum,dt3 cargo test -p snipr-core --release verifica_lojas -- --ignored --nocapture
    #[tokio::test]
    #[ignore]
    async fn verifica_lojas() {
        let _ = tracing_subscriber::fmt().with_env_filter(std::env::var("RUST_LOG").unwrap_or_else(|_| "warn".into())).try_init();
        let tmp = std::env::temp_dir().join("snipr-stage-test");
        let profile = std::env::var("SNIPR_TEST_PROFILE").map(std::path::PathBuf::from).unwrap_or_else(|_| tmp.join("profile"));
        std::fs::create_dir_all(&tmp).unwrap();
        let paths = crate::config::Paths { config_file: tmp.join("settings.json"), profile_dir: profile, work_dir: tmp.join("agents") };
        let engine = Engine::with_paths(paths).await.unwrap();
        let mut sites: Vec<Site> = crate::sites::default_sites();
        for (name, url) in crate::sites::suggestions().into_iter().map(|s| (s.name, s.url)).chain(
            [
                ("DT3", "https://dt3.com/"),
                ("Comfy", "https://www.comfy.com.br/"),
                ("FlexChair", "https://aflexchair.com.br/"),
                ("Sensetup", "https://sensetup.com.br/"),
                ("XT Racer", "https://www.xtracer.com.br/"),
            ]
            .map(|(a, b)| (a.to_string(), b.to_string())),
        ) {
            let domain = crate::sites::display_domain(&url);
            sites.push(Site {
                id: crate::sites::site_id_for(&domain),
                name,
                domain: url::Url::parse(&url).unwrap().host_str().unwrap().trim_start_matches("www.").into(),
                search_url: String::new(),
                home_url: url,
                login_url: None,
                requires_login: false,
                sells: String::new(),
                tips: String::new(),
                custom: true,
                manual: false,
                stage: None,
                api: None,
                json_url: String::new(),
            });
        }
        let only = std::env::var("SNIPR_TEST_SITES").unwrap_or_default();
        for site in sites {
            if !only.is_empty() && !only.split(',').any(|o| site.id.contains(o)) {
                continue;
            }
            let t = std::time::Instant::now();
            match engine.detect_stage(&site, "").await {
                Ok(i) => println!(
                    "{:<26} etapa {:?}{} em {:>4.1}s  {}",
                    site.name,
                    i.stage,
                    i.api.map(|a| format!(" ({a:?})")).unwrap_or_default(),
                    t.elapsed().as_secs_f64(),
                    i.search_url
                ),
                Err(e) => println!("{:<26} erro: {e:#}", site.name),
            }
        }
        engine.shutdown().await;
    }

    #[test]
    fn palavra_de_teste_vem_dos_cards() {
        let cards: Vec<Card> = ["Cadeira Ergonômica DT3 Alera", "Cadeira Gamer XT Racer", "Mesa Regulável Elétrica"]
            .iter()
            .map(|t| Card { title: t.to_string(), ..Default::default() })
            .collect();
        assert_eq!(probe_word(&cards).as_deref(), Some("cadeira"));
    }
}
