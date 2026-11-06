//! Diagnóstico: avalia expressões JS (argumentos 2..) no isolated world de uma página.
//!   cargo run -p snipr-browser --example debug -- <url> "<expr>" ...
use snipr_browser::{Browser, BrowserMode, LaunchOptions};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let url = args.next().expect("url");
    let b = Browser::launch(LaunchOptions {
        chrome_path: None,
        profile_dir: std::env::temp_dir().join("snipr-probe-profile"),
        mode: BrowserMode::Hidden,
        block_media: true,
        low_memory: true,
    })
    .await?;
    let p = b.new_page().await?;
    let nav = p.goto(&url).await?;
    println!("{} — {}", nav.url, nav.title);
    for expr in args {
        println!("> {expr}\n  {}", p.eval(&expr).await.map(|v| v.to_string()).unwrap_or_else(|e| e.to_string()));
    }
    b.close().await;
    Ok(())
}
