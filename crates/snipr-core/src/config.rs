use std::{collections::HashMap, path::PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use snipr_browser::BrowserMode;
use ts_rs::TS;

use crate::{
    sites::{default_enabled, default_sites, Site},
    types::{AgentMode, Condition},
};

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum AuthMode {
    #[default]
    Login,
    ApiKey,
    Oauth,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash, Default, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum Provider {
    #[default]
    Anthropic,
    Openai,
    Google,
    Xai,
    Openrouter,
    Ollama,
    Custom,
}

impl Provider {
    pub const ALL: [Provider; 7] =
        [Self::Anthropic, Self::Openai, Self::Google, Self::Xai, Self::Openrouter, Self::Ollama, Self::Custom];

    pub fn id(self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic",
            Self::Openai => "openai",
            Self::Google => "google",
            Self::Xai => "xai",
            Self::Openrouter => "openrouter",
            Self::Ollama => "ollama",
            Self::Custom => "custom",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Anthropic => "Anthropic",
            Self::Openai => "OpenAI",
            Self::Google => "Google",
            Self::Xai => "xAI",
            Self::Openrouter => "OpenRouter",
            Self::Ollama => "Ollama",
            Self::Custom => "endpoint personalizado",
        }
    }

    pub fn compat_base_url(self) -> Option<&'static str> {
        match self {
            Self::Anthropic | Self::Custom => None,
            Self::Openai => Some("https://api.openai.com/v1"),
            Self::Google => Some("https://generativelanguage.googleapis.com/v1beta/openai"),
            Self::Xai => Some("https://api.x.ai/v1"),
            Self::Openrouter => Some("https://openrouter.ai/api/v1"),
            Self::Ollama => Some("http://localhost:11434/v1"),
        }
    }

    pub fn needs_key(self) -> bool {
        !matches!(self, Self::Ollama)
    }

    pub fn key_env(self) -> &'static [&'static str] {
        match self {
            Self::Anthropic => &["ANTHROPIC_API_KEY"],
            Self::Openai => &["SNIPR_OPENAI_API_KEY", "OPENAI_API_KEY"],
            Self::Google => &["GEMINI_API_KEY", "GOOGLE_API_KEY"],
            Self::Xai => &["XAI_API_KEY"],
            Self::Openrouter => &["OPENROUTER_API_KEY"],
            Self::Ollama => &[],
            Self::Custom => &["SNIPR_CUSTOM_API_KEY"],
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, TS)]
#[serde(default)]
#[ts(export)]
pub struct AiSettings {
    pub mode: AuthMode,
    pub login_provider: Provider,
    pub key_provider: Provider,
    pub oauth_provider: Provider,
    pub endpoint: String,
    pub model: String,
    pub effort: String,
    pub cli_path: String,
}

impl Default for AiSettings {
    fn default() -> Self {
        Self {
            mode: AuthMode::Login,
            login_provider: Provider::Anthropic,
            key_provider: Provider::Anthropic,
            oauth_provider: Provider::Anthropic,
            endpoint: String::new(),
            model: String::new(),
            effort: String::new(),
            cli_path: String::new(),
        }
    }
}

impl AiSettings {
    pub fn provider(&self) -> Provider {
        match self.mode {
            AuthMode::Login => self.login_provider,
            AuthMode::ApiKey => self.key_provider,
            AuthMode::Oauth => self.oauth_provider,
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum Theme {
    System,
    #[default]
    Dark,
    Light,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum SessionState {
    Ok,
    Expired,
}

#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[serde(default)]
#[ts(export)]
pub struct Settings {
    pub theme: Theme,
    pub ai: AiSettings,
    pub agent_mode: AgentMode,
    #[ts(type = "\"hidden\" | \"headless\" | \"visible\"")]
    pub browser_mode: BrowserMode,
    pub block_media: bool,
    pub low_memory: bool,
    pub max_parallel: usize,
    pub max_steps: usize,
    #[ts(type = "number")]
    pub idle_close_secs: u64,
    pub chrome_path: String,
    pub enabled_sites: Vec<String>,
    pub conditions: Vec<Condition>,
    pub show_international: bool,
    pub custom_sites: Vec<Site>,
    pub removed_sites: Vec<String>,
    pub sessions: HashMap<String, SessionState>,
    pub login_overrides: HashMap<String, bool>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: Theme::Dark,
            ai: AiSettings::default(),
            agent_mode: AgentMode::Multi,
            browser_mode: BrowserMode::Hidden,
            block_media: true,
            low_memory: true,
            max_parallel: 4,
            max_steps: 60,
            idle_close_secs: 300,
            chrome_path: String::new(),
            enabled_sites: default_enabled(),
            conditions: vec![Condition::New],
            show_international: false,
            custom_sites: Vec::new(),
            removed_sites: Vec::new(),
            sessions: HashMap::new(),
            login_overrides: HashMap::new(),
        }
    }
}

impl Settings {
    /// Sites padrão + personalizados (um personalizado com o mesmo id substitui o padrão).
    pub fn all_sites(&self) -> Vec<Site> {
        let mut sites: Vec<Site> = default_sites().into_iter().filter(|s| !self.removed_sites.contains(&s.id)).collect();
        for c in &self.custom_sites {
            if let Some(s) = sites.iter_mut().find(|s| s.id == c.id) {
                *s = c.clone();
            } else {
                sites.push(c.clone());
            }
        }
        for s in &mut sites {
            if let Some(v) = self.login_overrides.get(&s.id) {
                s.requires_login = *v;
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
