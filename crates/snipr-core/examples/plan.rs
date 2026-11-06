//! Mostra o plano de busca gerado pela IA, sem abrir o navegador.
//!
//!   cargo run -p snipr-core --example plan -- "placa mãe gigabyte ou msi am5 até 1500" login openai

use snipr_core::{AuthMode, Engine, Provider};
use tokio_util::sync::CancellationToken;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let query = args.next().unwrap_or_else(|| "placa mãe gigabyte ou msi am5 wifi 4 slots ram ddr5 até 1500".into());
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        let engine = Engine::new().await?;
        let mut ai = engine.settings().ai;
        if let Some(mode) = args.next() {
            ai.mode = serde_json::from_value::<AuthMode>(serde_json::Value::String(mode))?;
        }
        if let Some(p) = args.next() {
            let p = serde_json::from_value::<Provider>(serde_json::Value::String(p))?;
            ai.login_provider = p;
            ai.key_provider = p;
        }
        let started = std::time::Instant::now();
        let plan = engine.plan_with(&ai, &query, CancellationToken::new()).await;
        println!("{}", serde_json::to_string_pretty(&plan)?);
        println!("({:.1}s)", started.elapsed().as_secs_f64());
        anyhow::Ok(())
    })
}
