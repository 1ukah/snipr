//! Escolha automática do "modelo rápido" (plano e filtro dos anúncios).
//!
//! As tarefas curtas do Snipr são classificação de texto com saída em JSON:
//! não precisam do modelo mais forte do provedor. Cada provedor tem uma
//! família pequena e capaz (Haiku, mini, Flash…); escolhemos a versão mais
//! nova dela entre os modelos que a conta lista. Nomes fixos envelheceriam a
//! cada lançamento; a família muda bem menos.

use crate::config::Provider;

/// Famílias aceitas, em ordem de preferência: (termos obrigatórios, termos proibidos).
fn tiers(p: Provider) -> &'static [(&'static [&'static str], &'static [&'static str])] {
    match p {
        Provider::Anthropic => &[(&["haiku"], &[]), (&["sonnet"], &[])],
        Provider::Openai => &[(&["gpt", "mini"], &[]), (&["mini"], &[])],
        Provider::Google => &[(&["gemini", "flash"], &["lite"]), (&["gemini", "flash-lite"], &[])],
        Provider::Xai => &[(&["grok", "fast"], &[]), (&["grok", "mini"], &[])],
        Provider::Openrouter => &[
            (&["google/gemini", "flash"], &["lite", ":free"]),
            (&["anthropic/claude", "haiku"], &[":free"]),
            (&["openai/gpt", "mini"], &[":free"]),
        ],
        Provider::Ollama | Provider::Custom => &[],
    }
}

/// Modelos que não servem para texto (voz, imagem, embeddings…).
const NOT_TEXT: &[&str] = &[
    "embed", "tts", "audio", "realtime", "transcribe", "image", "moderation", "whisper", "dall", "search", "live", "guard", "computer",
    "codex", "deep-research",
];

/// Números da versão no nome, sem datas (ex.: "gpt-5.4-mini-2026-03-01" → [5, 4]).
fn version(id: &str) -> Vec<u32> {
    // Para no primeiro número com cara de ano: o resto é a data do snapshot.
    id.split(|c: char| !c.is_ascii_digit()).filter_map(|n| n.parse::<u32>().ok()).take_while(|n| *n < 1000).collect()
}

fn unstable(id: &str) -> bool {
    ["preview", "exp", "beta", "alpha"].iter().any(|w| id.contains(w))
}

/// Modelo rápido recomendado entre `models`, ou `None` se nenhum servir
/// (nesse caso o Snipr usa o modelo principal).
pub fn pick_fast(provider: Provider, models: &[String]) -> Option<String> {
    for (must, never) in tiers(provider) {
        let mut found: Vec<&String> = models
            .iter()
            .filter(|m| {
                let l = m.to_lowercase();
                must.iter().all(|w| l.contains(w)) && !never.iter().any(|w| l.contains(w)) && !NOT_TEXT.iter().any(|w| l.contains(w))
            })
            .collect();
        found.sort_by(|a, b| {
            unstable(a)
                .cmp(&unstable(b))
                .then_with(|| version(b).cmp(&version(a)))
                .then_with(|| a.len().cmp(&b.len()))
        });
        if let Some(m) = found.first() {
            return Some((*m).clone());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn escolhe_a_familia_pequena_mais_nova() {
        let anthropic = ids(&["claude-opus-5-5", "claude-sonnet-5-5", "claude-haiku-4-5-20251001", "claude-haiku-5-5"]);
        assert_eq!(pick_fast(Provider::Anthropic, &anthropic).as_deref(), Some("claude-haiku-5-5"));

        let openai = ids(&["gpt-5.4", "gpt-5-mini-2025-08-07", "gpt-5.4-mini", "gpt-4o-mini-tts", "gpt-realtime-mini", "o4-mini"]);
        assert_eq!(pick_fast(Provider::Openai, &openai).as_deref(), Some("gpt-5.4-mini"));

        let google = ids(&["models/gemini-3.5-pro", "models/gemini-3.5-flash-lite", "models/gemini-3.5-flash", "models/gemini-3.6-flash-preview"]);
        assert_eq!(pick_fast(Provider::Google, &google).as_deref(), Some("models/gemini-3.5-flash"), "estável antes de preview");

        assert_eq!(pick_fast(Provider::Xai, &ids(&["grok-5", "grok-5-fast", "grok-4-fast"])).as_deref(), Some("grok-5-fast"));
        assert_eq!(pick_fast(Provider::Anthropic, &ids(&["claude-opus-5-5"])), None, "sem família rápida: usa o principal");
        assert_eq!(pick_fast(Provider::Ollama, &ids(&["llama4"])), None);
    }
}
