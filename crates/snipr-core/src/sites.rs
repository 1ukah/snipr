//! "Playbooks" de sites: URL de busca direta + dicas curtas para o agente.
//! Ir direto para a URL de busca economiza a maior parte dos passos.
//! Qualquer site pode ser adicionado nos ajustes (`custom_sites`).

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Como o Snipr lê os resultados de uma loja, do mais leve ao mais pesado.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum Stage {
    /// HTTP comum.
    A,
    /// HTTP com fingerprint TLS/HTTP2 de Chrome.
    B,
    /// Como B, com cookies que o Chrome obtém ao passar pela proteção anti-bot.
    C,
    /// Chrome abre a página e o Snipr lê os cards.
    D,
}

impl Stage {
    pub fn next(self) -> Option<Self> {
        match self {
            Self::A => Some(Self::B),
            Self::B => Some(Self::C),
            Self::C => Some(Self::D),
            Self::D => None,
        }
    }

    pub fn prev(self) -> Option<Self> {
        match self {
            Self::A => None,
            Self::B => Some(Self::A),
            Self::C => Some(Self::B),
            Self::D => Some(Self::C),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::A => "A",
            Self::B => "B",
            Self::C => "C",
            Self::D => "D",
        }
    }
}

/// API pública da plataforma da loja, usada no lugar da página de busca.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum Api {
    /// Catálogo VTEX (`/api/catalog_system/pub/products/search`).
    Vtex,
}

/// Resultado da verificação de uma loja, guardado nos ajustes.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, TS)]
#[ts(export)]
pub struct StageInfo {
    pub stage: Stage,
    #[serde(default)]
    pub api: Option<Api>,
    /// URL de busca encontrada na verificação (vazia = manter a da loja).
    #[serde(default)]
    pub search_url: String,
    /// Modelo do link dos produtos do JSON embutido na página (vazio = não usa).
    #[serde(default)]
    pub json_url: String,
    /// Segundos desde a época Unix.
    #[serde(default)]
    #[ts(type = "number")]
    pub checked_at: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, TS)]
#[ts(export)]
pub struct Site {
    pub id: String,
    pub name: String,
    pub domain: String,
    /// Placeholders: {q} (url-encoded), {q_plus} (espaços como +), {q_dash} (slug com hífens)
    #[serde(default)]
    pub search_url: String,
    #[serde(default)]
    pub home_url: String,
    #[serde(default)]
    pub login_url: Option<String>,
    #[serde(default)]
    pub requires_login: bool,
    /// "novos", "usados" ou "novos e usados"
    #[serde(default)]
    pub sells: String,
    #[serde(default)]
    pub tips: String,
    #[serde(default)]
    pub custom: bool,
    #[serde(default)]
    pub manual: bool,
    /// Etapa usada para ler a loja. `None` = ainda não verificada (usa o Chrome).
    #[serde(default)]
    pub stage: Option<Stage>,
    #[serde(default)]
    pub api: Option<Api>,
    /// Modelo do link dos produtos do JSON embutido (ver `StageInfo::json_url`).
    #[serde(default)]
    pub json_url: String,
}

#[derive(Clone, Debug, Serialize, TS)]
#[ts(export)]
pub struct Suggestion {
    pub name: String,
    pub url: String,
    pub domain: String,
}

impl Site {
    pub fn build_search_url(&self, query: &str) -> String {
        let q = query.trim();
        let enc = urlencoding::encode(q).into_owned();
        let plus = q.split_whitespace().map(|w| urlencoding::encode(w).into_owned()).collect::<Vec<_>>().join("+");
        let dash = q
            .to_lowercase()
            .split_whitespace()
            .map(|w| urlencoding::encode(w).into_owned())
            .collect::<Vec<_>>()
            .join("-");
        self.search_url.replace("{q}", &enc).replace("{q_plus}", &plus).replace("{q_dash}", &dash)
    }

    pub fn home(&self) -> String {
        if self.home_url.is_empty() {
            format!("https://www.{}/", self.domain.trim_start_matches("www."))
        } else {
            self.home_url.clone()
        }
    }

    pub fn matches_host(&self, host: &str) -> bool {
        let d = self.domain.trim_start_matches("www.");
        host == d || host.ends_with(&format!(".{d}"))
    }

    /// Origem da loja (`https://host`), para APIs da plataforma.
    pub fn origin(&self) -> String {
        url::Url::parse(&self.home())
            .map(|u| u.origin().ascii_serialization())
            .unwrap_or_else(|_| format!("https://{}", self.domain))
    }
}

#[allow(clippy::too_many_arguments)] // tabela de dados, mais legível assim
fn site(
    id: &str,
    name: &str,
    domain: &str,
    search_url: &str,
    sells: &str,
    requires_login: bool,
    login_url: Option<&str>,
    stage: Stage,
    tips: &str,
) -> Site {
    Site {
        id: id.into(),
        name: name.into(),
        domain: domain.into(),
        search_url: search_url.into(),
        login_url: login_url.map(String::from),
        requires_login,
        sells: sells.into(),
        tips: tips.into(),
        home_url: String::new(),
        custom: false,
        manual: false,
        stage: Some(stage),
        api: None,
        json_url: String::new(),
    }
}

/// Etapas medidas nas lojas padrão: A/B leem o HTML por HTTP; Magalu (C)
/// precisa dos cookies que o Chrome obtém; Mercado Livre recusa qualquer
/// cliente que não seja um navegador, mesmo com cookies (D).
pub fn default_sites() -> Vec<Site> {
    vec![
        site(
            "mercadolivre", "Mercado Livre", "mercadolivre.com.br",
            "https://lista.mercadolivre.com.br/{q_dash}", "novos e usados", false,
            Some("https://www.mercadolivre.com.br/login"), Stage::D,
            "Cards mostram preço atual e às vezes o riscado (ignore o riscado). Cards de usado têm o texto 'Usado'. Há filtro de 'Condição' na lateral e ordenação por 'Menor preço'.",
        ),
        site(
            "olx", "OLX", "olx.com.br",
            "https://www.olx.com.br/brasil?q={q_plus}", "usados (majoritariamente)", false,
            Some("https://conta.olx.com.br/acesso"), Stage::B,
            "Busca nacional. Cada card mostra cidade/UF. Há opção de ordenar por menor preço. Muitos anúncios genéricos: confira o modelo no título.",
        ),
        site(
            "facebook", "Facebook Marketplace", "facebook.com",
            "https://www.facebook.com/marketplace/search/?query={q}", "usados (majoritariamente)", true,
            Some("https://www.facebook.com/login"), Stage::D,
            "Exige login. Resultados carregam ao rolar: use scroll 1-2 vezes antes de extract_listings. A região é a da conta. Não abra conversas com vendedores.",
        ),
        site(
            "amazon", "Amazon", "amazon.com.br",
            "https://www.amazon.com.br/s?k={q_plus}", "novos (e usados via outras ofertas)", false,
            Some("https://www.amazon.com.br/ap/signin"), Stage::A,
            "Ignore resultados 'Patrocinado' de outros modelos. O preço à vista aparece em destaque; não use o valor da parcela.",
        ),
        Site {
            // O HTML traz só os 10 primeiros cards; os demais estão no JSON da página.
            json_url: "https://www.kabum.com.br/produto/{code}/{friendlyName}".into(),
            ..site(
                "kabum", "KaBuM!", "kabum.com.br",
                "https://www.kabum.com.br/busca/{q_dash}", "novos", false, None, Stage::A,
                "Loja de novos. Mostra preço à vista (PIX) e preço parcelado: registre o à vista.",
            )
        },
        site(
            "terabyte", "Terabyte", "terabyteshop.com.br",
            "https://www.terabyteshop.com.br/busca?str={q_plus}", "novos", false, None, Stage::B,
            "Loja de novos. Preço à vista em destaque. Produtos esgotados aparecem como 'Avise-me': não registre esgotados.",
        ),
        site(
            "pichau", "Pichau", "pichau.com.br",
            "https://www.pichau.com.br/search?q={q}", "novos", false, None, Stage::D,
            "Loja de novos. Preço à vista (PIX) em destaque. Ignore produtos esgotados.",
        ),
        site(
            "enjoei", "Enjoei", "enjoei.com.br",
            "https://www.enjoei.com.br/s?q={q}", "usados", false,
            Some("https://www.enjoei.com.br/login"), Stage::D,
            "Usados, muitos itens de moda: confira se o título é mesmo o produto pedido.",
        ),
        site(
            "magalu", "Magazine Luiza", "magazineluiza.com.br",
            "https://www.magazineluiza.com.br/busca/{q_plus}/", "novos", false, None, Stage::C,
            "Marketplace de novos. Registre o preço à vista/PIX.",
        ),
        site(
            "shopee", "Shopee", "shopee.com.br",
            "https://shopee.com.br/search?keyword={q}", "novos", true,
            Some("https://shopee.com.br/buyer/login"), Stage::D,
            "Costuma exigir login para buscar. Muitos anúncios de peças/capas: filtre com cuidado.",
        ),
    ]
}

pub fn default_enabled() -> Vec<String> {
    ["mercadolivre", "amazon", "kabum", "terabyte", "pichau", "magalu"]
        .into_iter()
        .map(String::from)
        .collect()
}

pub fn suggestions() -> Vec<Suggestion> {
    [
        ("Americanas", "https://www.americanas.com.br/"),
        ("Carrefour", "https://www.carrefour.com.br/"),
        ("AliExpress", "https://pt.aliexpress.com/"),
        ("Pernambucanas", "https://www.pernambucanas.com.br/"),
        ("Fast Shop", "https://site.fastshop.com.br/"),
        ("Kalunga", "https://www.kalunga.com.br/"),
        ("Chipart", "https://www.chipart.com.br/"),
        ("Logitech Store", "https://www.logitechstore.com.br/"),
        ("HP Store", "https://www.hp.com/br-pt/shop/"),
        ("Motorola", "https://www.motorola.com.br/"),
        ("LG", "https://www.lg.com/br/"),
        ("Samsung", "https://www.samsung.com/br/"),
        ("Apple Store", "https://www.apple.com/br/store"),
        ("Dell", "https://www.dell.com/pt-br"),
        ("Lenovo", "https://www.lenovo.com/br/pt/"),
        ("Acer Store", "https://br-store.acer.com/"),
        ("ASUS Store", "https://www.asus.com/br/store/"),
        ("Avell", "https://avell.com.br/"),
    ]
    .into_iter()
    .map(|(name, url)| Suggestion { name: name.into(), url: url.into(), domain: display_domain(url) })
    .collect()
}

pub fn display_domain(url: &str) -> String {
    let s = url.trim().trim_start_matches("https://").trim_start_matches("http://");
    let s = s.trim_start_matches("www.");
    s.split(['?', '#']).next().unwrap_or_default().trim_end_matches('/').to_string()
}

pub fn site_id_for(domain: &str) -> String {
    let id: String = domain.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).collect();
    format!("x-{}", id.trim_matches('-'))
}
