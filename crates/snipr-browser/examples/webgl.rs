//! Mostra o que um site enxerga do WebGL (sinal comum de detecção de bot).
use snipr_browser::{Browser, BrowserMode, LaunchOptions};
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let b = Browser::launch(LaunchOptions { chrome_path: None, profile_dir: std::env::temp_dir().join("snipr-probe-profile"), mode: BrowserMode::Hidden, block_media: true, low_memory: true }).await?;
    let p = b.new_page().await?;
    p.goto("https://example.com/").await?;
    let v = p.eval("(() => { const c = document.createElement('canvas'); const g = c.getContext('webgl'); if (!g) return 'sem WebGL'; const e = g.getExtension('WEBGL_debug_renderer_info'); return e ? g.getParameter(e.UNMASKED_VENDOR_WEBGL) + ' | ' + g.getParameter(e.UNMASKED_RENDERER_WEBGL) : 'sem debug info'; })()").await?;
    println!("WebGL: {}", v.as_str().unwrap_or_default());
    b.close().await;
    Ok(())
}
