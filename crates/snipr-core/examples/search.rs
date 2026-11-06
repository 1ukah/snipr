//! Frontend mínimo de linha de comando: dispara uma busca pelo `Engine`
//! exatamente como a GUI faz (chamando `start_search` de uma thread SEM
//! runtime Tokio) e imprime os eventos. Usa os ajustes e segredos reais.
//!
//!   cargo run -p snipr-core --example search -- "cadeira dt3 n10 xl" mercadolivre,olx

use std::time::Duration;

use snipr_core::{AgentMode, ConditionFilter, Engine, SearchEvent, SearchRequest};

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let query = args.next().unwrap_or_else(|| "cadeira dt3 n10 xl".into());
    let sites: Vec<String> = args.next().map(|s| s.split(',').map(String::from).collect()).unwrap_or_default();

    let rt = tokio::runtime::Runtime::new()?;
    let engine = rt.block_on(Engine::new())?;
    println!("provedor: {:?}", engine.settings().provider);

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    // Thread sem runtime, como a thread da interface do Tauri.
    let e2 = engine.clone();
    let job = std::thread::spawn(move || {
        e2.start_search(
            SearchRequest {
                query,
                condition: ConditionFilter::Any,
                sites,
                mode: AgentMode::Single,
                max_price: None,
                notes: None,
            },
            tx,
        )
    })
    .join()
    .expect("thread")?;
    println!("job {job} iniciado");

    rt.block_on(async {
        while let Some(ev) = tokio::time::timeout(Duration::from_secs(600), rx.recv()).await.ok().flatten() {
            match &ev {
                SearchEvent::Log { message, .. } => println!("  · {message}"),
                SearchEvent::Listing { listing, .. } => {
                    println!("  $ R$ {:.2} [{:?}] {} — {}", listing.price, listing.condition, listing.title, listing.site_name)
                }
                SearchEvent::AgentFinished { ok, summary, .. } => println!("  {} {summary}", if *ok { "✓" } else { "✗" }),
                SearchEvent::Finished { total, elapsed_ms, .. } => {
                    println!("fim: {total} anúncio(s) em {:.1}s", *elapsed_ms as f64 / 1000.0);
                    break;
                }
                other => println!("  {other:?}"),
            }
        }
        engine.shutdown().await;
    });
    Ok(())
}
