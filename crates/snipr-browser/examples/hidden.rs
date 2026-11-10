//! Verifica o modo oculto: abre janelas de agentes, mostra uma janela (como no
//! login) e oculta de novo, informando o estado do app em cada etapa.
//!
//!   cargo run -p snipr-browser --example hidden --release -- [hidden|visible]

use std::time::Duration;

use snipr_browser::{Browser, BrowserMode, LaunchOptions};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let b = Browser::launch(LaunchOptions {
        chrome_path: None,
        profile_dir: std::env::temp_dir().join("snipr-hidden-test"),
        mode: match std::env::args().nth(1).as_deref() {
            Some("visible") => BrowserMode::Visible,
            _ => BrowserMode::Hidden,
        },
        block_media: true,
        low_memory: true,
    })
    .await?;
    println!("lançado; app oculto: {:?}", b.app_hidden());
    let p1 = b.new_page().await?;
    let p2 = b.new_page().await?;
    println!("janelas criadas; app oculto: {:?}", b.app_hidden());
    p1.goto("https://www.kabum.com.br/busca/placa-mae-am5").await?;
    println!("p1 carregou; app oculto: {:?}", b.app_hidden());
    p2.goto("https://lista.mercadolivre.com.br/placa-mae-am5").await?;
    println!("duas janelas abertas; app oculto: {:?}", b.app_hidden());
    let cards = p1.extract_cards(5).await?;
    println!("anúncios lidos com a janela oculta: {}", cards.iter().map(|c| c.title.as_str()).collect::<Vec<_>>().join(" | "));

    p2.set_visible(true).await?;
    tokio::time::sleep(Duration::from_secs(2)).await;
    println!("janela de login mostrada; app oculto: {:?}", b.app_hidden());

    p2.set_visible(false).await?;
    tokio::time::sleep(Duration::from_secs(2)).await;
    println!("ocultado de novo; app oculto: {:?}", b.app_hidden());
    let again = p1.extract_cards(3).await?;
    println!("anúncios lidos depois: {}", again.len());

    let iso = b.new_isolated_page().await?;
    let nav = iso.goto("https://www.facebook.com/marketplace/").await?;
    let wall = iso.eval("!![...document.querySelectorAll('input[type=password]')].length || /login/.test(location.pathname)").await?;
    println!("página isolada (sem cookies): {} | tela de login: {wall}; app oculto: {:?}", nav.url, b.app_hidden());
    iso.close().await;

    p1.close().await;
    p2.close().await;
    b.close().await;
    Ok(())
}
