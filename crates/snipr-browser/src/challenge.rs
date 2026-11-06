//! Tratamento automático de desafios anti-bot.
//!
//! Estratégia: num Chrome real com IP residencial o desafio gerenciado do
//! Cloudflare costuma passar sozinho em poucos segundos. Se o checkbox do
//! Turnstile aparecer, clicamos nele com o mouse "humano". Só se nada disso
//! funcionar devolvemos `Unsolved`, e o app pede ajuda ao usuário.

use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{json, Value};

use crate::{human, page::Page};

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(tag = "status", content = "kind", rename_all = "snake_case")]
pub enum ChallengeOutcome {
    None,
    Solved(String),
    Unsolved(String),
}

impl ChallengeOutcome {
    pub fn describe(&self) -> String {
        match self {
            Self::None => "nenhum".into(),
            Self::Solved(k) => format!("{k} resolvido automaticamente"),
            Self::Unsolved(k) => format!("{k} NÃO resolvido"),
        }
    }
}

pub async fn handle(page: &Page) -> ChallengeOutcome {
    let Some(kind) = page.detect_challenge().await else {
        return ChallengeOutcome::None;
    };
    tracing::info!(%kind, "desafio anti-bot detectado");
    // Bloqueio na borda (403/429): não há o que clicar nem esperar aqui;
    // quem chama decide se aguarda e tenta de novo.
    if kind == "blocked" {
        return ChallengeOutcome::Unsolved(kind);
    }
    let start = Instant::now();
    let mut clicks = 0;
    let mut next_click = Duration::from_secs(4);
    while start.elapsed() < Duration::from_secs(30) {
        tokio::time::sleep(Duration::from_millis(900)).await;
        match page.detect_challenge().await {
            None => {
                page.wait_ready(Duration::from_secs(8)).await;
                return ChallengeOutcome::Solved(kind);
            }
            Some(k) if k == "cloudflare" && start.elapsed() >= next_click && clicks < 3 => {
                if let Some((x, y)) = find_turnstile_checkbox(page).await {
                    tracing::info!(x, y, "clicando no Turnstile");
                    let _ = page.click_at(x, y).await;
                    clicks += 1;
                }
                next_click = start.elapsed() + Duration::from_secs(human::rnd(5, 8));
            }
            Some(_) => {}
        }
    }
    ChallengeOutcome::Unsolved(kind)
}

/// Localiza o iframe do Turnstile atravessando shadow roots fechados
/// (DOM.getDocument com pierce) e devolve a posição do checkbox.
async fn find_turnstile_checkbox(page: &Page) -> Option<(f64, f64)> {
    let doc = page.cmd("DOM.getDocument", json!({ "depth": -1, "pierce": true })).await.ok()?;
    let node = find_iframe(&doc["root"])?;
    let model = page.cmd("DOM.getBoxModel", json!({ "backendNodeId": node })).await.ok()?;
    let q = model["model"]["content"].as_array()?;
    let nums: Vec<f64> = q.iter().filter_map(Value::as_f64).collect();
    if nums.len() < 8 {
        return None;
    }
    let (x1, y1, x3, y3) = (nums[0], nums[1], nums[4], nums[5]);
    let (w, h) = (x3 - x1, y3 - y1);
    if w < 30.0 || h < 30.0 {
        return None;
    }
    // O checkbox fica à esquerda do widget, centralizado na vertical.
    let x = x1 + (w * 0.09).clamp(18.0, 34.0) + human::jitter(4.0);
    let y = y1 + h / 2.0 + human::jitter(4.0);
    Some((x, y))
}

fn find_iframe(node: &Value) -> Option<i64> {
    if node["nodeName"].as_str() == Some("IFRAME") {
        if let Some(attrs) = node["attributes"].as_array() {
            let mut it = attrs.iter();
            while let (Some(k), Some(v)) = (it.next(), it.next()) {
                if k.as_str() == Some("src") && v.as_str().is_some_and(|s| s.contains("challenges.cloudflare.com")) {
                    return node["backendNodeId"].as_i64();
                }
            }
        }
    }
    for key in ["children", "shadowRoots"] {
        if let Some(arr) = node[key].as_array() {
            for c in arr {
                if let Some(id) = find_iframe(c) {
                    return Some(id);
                }
            }
        }
    }
    if node.get("contentDocument").is_some() {
        return find_iframe(&node["contentDocument"]);
    }
    None
}
