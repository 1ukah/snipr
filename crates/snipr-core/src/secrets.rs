//! Cofre de segredos.
//!
//! - Guardado no cofre nativo do SO: Credential Manager (Windows),
//!   Keychain (macOS), Secret Service (Linux). Criptografado pelo sistema.
//! - Somente escrita a partir da interface: não existe comando para ler um
//!   segredo de volta. A UI só pergunta "existe?" e pode trocar/remover.
//! - Também aceita variáveis de ambiente / arquivo `.env` como alternativa.

use anyhow::{bail, Result};

const SERVICE: &str = "snipr";

/// Segredos conhecidos e a variável de ambiente equivalente.
pub const KNOWN: &[(&str, &[&str])] = &[
    ("claude_code_oauth_token", &["CLAUDE_CODE_OAUTH_TOKEN"]),
    ("anthropic_api_key", &["ANTHROPIC_API_KEY"]),
    ("openai_api_key", &["SNIPR_OPENAI_API_KEY", "OPENAI_API_KEY"]),
];

pub fn is_known(name: &str) -> bool {
    KNOWN.iter().any(|(n, _)| *n == name) || name.starts_with("site:")
}

fn env_value(name: &str) -> Option<String> {
    KNOWN
        .iter()
        .find(|(n, _)| *n == name)
        .and_then(|(_, vars)| vars.iter().find_map(|v| std::env::var(v).ok()))
        .filter(|v| !v.trim().is_empty())
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

pub fn site_key(domain: &str, field: &str) -> String {
    format!("site:{domain}:{field}")
}

pub fn normalize_domain(input: &str) -> String {
    let s = input.trim().to_lowercase();
    let s = s.trim_start_matches("https://").trim_start_matches("http://");
    let s = s.split('/').next().unwrap_or_default();
    s.trim_start_matches("www.").to_string()
}

#[cfg(test)]
mod tests {
    #[test]
    fn cofre_do_sistema_grava_le_e_apaga() {
        let name = super::site_key("teste.snipr.invalid", "password");
        super::set(&name, "  s3nh@-de-teste  ").unwrap();
        assert!(super::has(&name));
        assert_eq!(super::reveal(&name).as_deref(), Some("s3nh@-de-teste"));
        super::delete(&name).unwrap();
        assert!(!super::has(&name));
        assert!(super::set("nome_desconhecido", "x").is_err());
    }
}
