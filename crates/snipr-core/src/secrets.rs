//! Cofre de segredos.
//!
//! - Guardado no cofre nativo do SO: Credential Manager (Windows),
//!   Keychain (macOS), Secret Service (Linux). Criptografado pelo sistema.
//! - Somente escrita a partir da interface: não existe comando para ler um
//!   segredo de volta. A UI só pergunta "existe?" e pode trocar/remover.
//! - Também aceita variáveis de ambiente / arquivo `.env` como alternativa.

use anyhow::{bail, Result};

use crate::config::Provider;

const SERVICE: &str = "snipr";

pub fn api_key_name(p: Provider) -> String {
    format!("api_key:{}", p.id())
}

pub fn oauth_name(p: Provider) -> String {
    format!("oauth:{}", p.id())
}

fn parse(name: &str) -> Option<(&str, Provider)> {
    let (kind, id) = name.split_once(':')?;
    let p = Provider::ALL.into_iter().find(|p| p.id() == id)?;
    matches!(kind, "api_key" | "oauth").then_some((kind, p))
}

pub fn is_known(name: &str) -> bool {
    parse(name).is_some()
}

fn env_value(name: &str) -> Option<String> {
    let vars: &[&str] = match parse(name)? {
        ("api_key", p) => p.key_env(),
        ("oauth", Provider::Anthropic) => &["CLAUDE_CODE_OAUTH_TOKEN"],
        _ => &[],
    };
    vars.iter().find_map(|v| std::env::var(v).ok()).filter(|v| !v.trim().is_empty())
}

fn entry(name: &str) -> Result<keyring::Entry> {
    Ok(keyring::Entry::new(SERVICE, name)?)
}

pub fn set(name: &str, value: &str) -> Result<()> {
    if !is_known(name) {
        bail!("segredo desconhecido: {name}");
    }
    let v = value.trim();
    if v.is_empty() {
        bail!("valor vazio");
    }
    entry(name)?.set_password(v)?;
    Ok(())
}

pub fn has(name: &str) -> bool {
    entry(name).and_then(|e| Ok(e.get_password()?)).is_ok() || env_value(name).is_some()
}

pub fn delete(name: &str) -> Result<()> {
    match entry(name)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(e.into()),
    }
}

/// Lê o valor. Uso exclusivo do backend (ex.: injetar no processo do Claude
/// Code ou digitar no browser via `type_secret`). Nunca exposto para a UI
/// nem para o modelo de IA.
pub(crate) fn reveal(name: &str) -> Option<String> {
    entry(name).ok().and_then(|e| e.get_password().ok()).or_else(|| env_value(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nomes_validos() {
        assert!(is_known("api_key:openai"));
        assert!(is_known("oauth:anthropic"));
        assert!(!is_known("api_key:desconhecido"));
        assert!(!is_known("site:facebook.com:password"));
    }

    #[test]
    #[ignore]
    fn cofre_do_sistema_grava_le_e_apaga() {
        let name = api_key_name(Provider::Custom);
        set(&name, "  chave-de-teste  ").unwrap();
        assert!(has(&name));
        assert_eq!(reveal(&name).as_deref(), Some("chave-de-teste"));
        delete(&name).unwrap();
        assert!(set("nome_desconhecido", "x").is_err());
    }
}
