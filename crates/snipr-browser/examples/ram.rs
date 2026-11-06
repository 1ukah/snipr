//! Mede a RAM total do Chrome com N agentes (abas) em paralelo.
//!   cargo run -p snipr-browser --example ram --release
use snipr_browser::{Browser, BrowserMode, LaunchOptions};

fn chrome_ram_mb(profile: &str) -> f64 {
    let ps = format!(
        "(Get-CimInstance Win32_Process -Filter \"Name='chrome.exe'\" | Where-Object {{ $_.CommandLine -like '*{profile}*' }} | ForEach-Object {{ (Get-Process -Id $_.ProcessId -ErrorAction SilentlyContinue).PrivateMemorySize64 }} | Measure-Object -Sum).Sum"
    );
    let out = std::process::Command::new("powershell").args(["-NoProfile", "-Command", &ps]).output().unwrap();
    String::from_utf8_lossy(&out.stdout).trim().parse::<f64>().unwrap_or(0.0) / 1_048_576.0
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let urls = [
        "https://lista.mercadolivre.com.br/cadeira-dt3-n10-xl",
        "https://www.olx.com.br/brasil?q=cadeira+dt3+n10",
        "https://www.kabum.com.br/busca/cadeira-dt3",
        "https://www.pichau.com.br/search?q=cadeira%20dt3",
    ];
    let profile = std::env::temp_dir().join("snipr-ram-profile");
    let pstr = profile.display().to_string();
    for (block, low) in [(true, true)] {
        let b = Browser::launch(LaunchOptions { chrome_path: None, profile_dir: profile.clone(), mode: BrowserMode::Hidden, block_media: block, low_memory: low }).await?;
        println!("\nbloquear mídia={block} pouca memória={low}");
        println!("  Chrome ocioso: {:.0} MB", chrome_ram_mb(&pstr));
        let mut pages = vec![];
        for u in urls { pages.push(b.new_page().await?); let _ = u; }
        let futs = pages.iter().zip(urls).map(|(p, u)| p.goto(u));
        futures_util::future::join_all(futs).await;
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        println!("  {} abas carregadas em paralelo: {:.0} MB", urls.len(), chrome_ram_mb(&pstr));
        if block {
            let out = std::process::Command::new("powershell").args(["-NoProfile", "-File", &std::env::var("BREAKDOWN").unwrap_or_default(), &pstr]).output().unwrap();
            println!("{}", String::from_utf8_lossy(&out.stdout));
        }
        b.close().await;
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    }
    Ok(())
}
