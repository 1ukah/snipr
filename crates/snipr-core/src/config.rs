use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use snipr_browser::BrowserMode;

use crate::sites::{default_enabled, default_sites, Site};

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    /// Executa o Claude Code local (`claude -p`) — usa a assinatura via OAuth.
    ClaudeCode,
    /// API da Anthropic com API key.
    Anthropic,
    /// Qualquer API compatível com OpenAI (OpenAI, Grok, Gemini, OpenRouter, Ollama...).
    OpenaiCompat,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub provider: ProviderKind,
    /// Vazio = padrão do provedor.
    pub model: String,
    /// low | medium | high | xhigh | max. Vazio = padrão.
    pub effort: String,
    pub openai_base_url: String,
    pub browser_mode: BrowserMode,
    pub block_media: bool,
    pub low_memory: bool,
    /// Quantos agentes/abas simultâneos no modo multi-agente.
    pub max_parallel: usize,
    pub max_steps: usize,
    /// Fecha o Chrome após N segundos sem buscas (libera RAM). 0 = nunca.
    pub idle_close_secs: u64,
    pub chrome_path: String,
    pub claude_path: String,
    pub enabled_sites: Vec<String>,
    pub custom_sites: Vec<Site>,
    /// Domínios com credenciais no cofre (os valores nunca ficam aqui).
    pub credential_domains: Vec<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            provider: ProviderKind::ClaudeCode,
            model: String::new(),
            effort: String::new(),
            openai_base_url: "https://api.openai.com/v1".into(),
            browser_mode: BrowserMode::Hidden,
            block_media: true,
            low_memory: true,
            max_parallel: 3,
            max_steps: 45,
            idle_close_secs: 180,
            chrome_path: String::new(),
            claude_path: String::new(),
            enabled_sites: default_enabled(),
            custom_sites: Vec::new(),
            credential_domains: Vec::new(),
        }
    }
}

impl Settings {
    /// Sites padrão + personalizados (um personalizado com o mesmo id substitui o padrão).
    pub fn all_sites(&self) -> Vec<Site> {
        let mut sites = default_sites();
        for c in &self.custom_sites {
            if let Some(s) = sites.iter_mut().find(|s| s.id == c.id) {
                *s = c.clone();
            } else {
                sites.push(c.clone());
            }
        }
        sites
    }
}

#[derive(Clone, Debug)]
pub struct Paths {
    pub config_file: PathBuf,
    pub profile_dir: PathBuf,
    pub work_dir: PathBuf,
}

impl Paths {
    pub fn resolve() -> Result<Self> {
        let config_dir = dirs::config_dir().context("sem diretório de configuração")?.join("snipr");
        let data_dir = dirs::data_local_dir().context("sem diretório de dados")?.join("snipr");
        std::fs::create_dir_all(&config_dir)?;
        std::fs::create_dir_all(&data_dir)?;
        Ok(Self {
            config_file: config_dir.join("settings.json"),
            profile_dir: data_dir.join("chrome-profile"),
            work_dir: data_dir.join("agents"),
        })
    }

    pub fn load_settings(&self) -> Settings {
        std::fs::read_to_string(&self.config_file)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save_settings(&self, s: &Settings) -> Result<()> {
        let tmp = self.config_file.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(s)?)?;
        std::fs::rename(&tmp, &self.config_file)?;
        Ok(())
    }
}
