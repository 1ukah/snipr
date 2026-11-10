//! Teste de fumaça da camada de navegador, sem IA.
//!
//!   cargo run -p snipr-browser --example probe -- "https://lista.mercadolivre.com.br/cadeira-dt3-n10-xl" [hidden|headless|visible]
//!
//! Abre a URL com o perfil de teste, trata desafio anti-bot e imprime os
//! anúncios extraídos e um trecho do snapshot. Também roda uma checagem
//! básica de sinais de automação.

use std::time::Instant;

use snipr_browser::{Browser, BrowserMode, LaunchOptions};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let url = std::env::args().nth(1).unwrap_or_else(|| "https://lista.mercadolivre.com.br/cadeira-dt3-n10-xl".into());
    let mode = match std::env::args().nth(2).as_deref() {
        Some("headless") => BrowserMode::Headless,
        Some("visible") => BrowserMode::Visible,
        _ => BrowserMode::Hidden,
    };
    let profile = std::env::temp_dir().join("snipr-probe-profile");
    let t = Instant::now();
    let browser = Browser::launch(LaunchOptions {
        chrome_path: None,
        profile_dir: profile,
        mode,
        block_media: std::env::var("BLOCK").map(|v| v != "0").unwrap_or(true),
        low_memory: true,
    })
    .await?;
    println!("navegador: {} ({:?}) em {:?}", browser.version, mode, t.elapsed());

    let page = browser.new_page().await?;
    let t = Instant::now();
    let nav = page.goto(&url).await?;
    println!("aberto em {:?}: {} — {} | desafio: {}", t.elapsed(), nav.url, nav.title, nav.challenge.describe());

    let signals = page
        .eval(
            "JSON.stringify({ webdriver: navigator.webdriver, ua: navigator.userAgent, \
             langs: navigator.languages, plugins: navigator.plugins.length, \
             vis: document.visibilityState, focus: document.hasFocus(), \
             w: innerWidth, h: innerHeight })",
        )
        .await?;
    println!("sinais: {}", signals.as_str().unwrap_or_default());

    let t = Instant::now();
    let cards = page.extract_cards(15).await?;
    println!("\n--- extract_cards: {} em {:?} ---", cards.len(), t.elapsed());
    for c in &cards {
        println!("{} | antes {} | {} | {}", c.prices.join(" / "), c.old.join(", "), c.title, c.url);
    }

    let snap = page.snapshot(25, None).await?;
    println!("\n--- snapshot (25 itens) ---\n{snap}");

    page.close().await;
    browser.close().await;
    Ok(())
}
