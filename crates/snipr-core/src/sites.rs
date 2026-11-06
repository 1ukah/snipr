//! "Playbooks" de sites: URL de busca direta + dicas curtas para o agente.
//! Ir direto para a URL de busca economiza a maior parte dos passos.
//! Qualquer site pode ser adicionado nos ajustes (`custom_sites`).

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Site {
    pub id: String,
    pub name: String,
    pub domain: String,
    /// Placeholders: {q} (url-encoded), {q_plus} (espaços como +), {q_dash} (slug com hífens)
    pub search_url: String,
    #[serde(default)]
    pub login_url: Option<String>,
    #[serde(default)]
    pub requires_login: bool,
    /// "novos", "usados" ou "novos e usados"
    #[serde(default)]
    pub sells: String,
    #[serde(default)]
    pub tips: String,
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

    pub fn matches_host(&self, host: &str) -> bool {
        let d = self.domain.trim_start_matches("www.");
        host == d || host.ends_with(&format!(".{d}"))
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
    }
}

pub fn default_sites() -> Vec<Site> {
    vec![
        site(
            "mercadolivre", "Mercado Livre", "mercadolivre.com.br",
            "https://lista.mercadolivre.com.br/{q_dash}", "novos e usados", false,
            Some("https://www.mercadolivre.com.br/login"),
            "Cards mostram preço atual e às vezes o riscado (ignore o riscado). Cards de usado têm o texto 'Usado'. Há filtro de 'Condição' na lateral e ordenação por 'Menor preço'.",
        ),
        site(
            "olx", "OLX", "olx.com.br",
            "https://www.olx.com.br/brasil?q={q_plus}", "usados (majoritariamente)", false,
            Some("https://conta.olx.com.br/acesso"),
            "Busca nacional. Cada card mostra cidade/UF. Há opção de ordenar por menor preço. Muitos anúncios genéricos: confira o modelo no título.",
        ),
        site(
            "facebook", "Facebook Marketplace", "facebook.com",
            "https://www.facebook.com/marketplace/search/?query={q}", "usados (majoritariamente)", true,
            Some("https://www.facebook.com/login"),
            "Exige login. Resultados carregam ao rolar: use scroll 1-2 vezes antes de extract_listings. A região é a da conta. Não abra conversas com vendedores.",
        ),
        site(
            "amazon", "Amazon", "amazon.com.br",
            "https://www.amazon.com.br/s?k={q_plus}", "novos (e usados via outras ofertas)", false,
            Some("https://www.amazon.com.br/ap/signin"),
            "Ignore resultados 'Patrocinado' de outros modelos. O preço à vista aparece em destaque; não use o valor da parcela.",
        ),
        site(
            "kabum", "KaBuM!", "kabum.com.br",
            "https://www.kabum.com.br/busca/{q_dash}", "novos", false, None,
            "Loja de novos. Mostra preço à vista (PIX) e preço parcelado: registre o à vista e mencione nas notas.",
        ),
        site(
            "terabyte", "Terabyte", "terabyteshop.com.br",
            "https://www.terabyteshop.com.br/busca?str={q_plus}", "novos", false, None,
            "Loja de novos. Preço à vista em destaque. Produtos esgotados aparecem como 'Avise-me': não registre esgotados.",
        ),
        site(
            "pichau", "Pichau", "pichau.com.br",
            "https://www.pichau.com.br/search?q={q}", "novos", false, None,
            "Loja de novos. Preço à vista (PIX) em destaque. Ignore produtos esgotados.",
        ),
        site(
            "enjoei", "Enjoei", "enjoei.com.br",
            "https://www.enjoei.com.br/s?q={q}", "usados", false,
            Some("https://www.enjoei.com.br/login"),
            "Usados, muitos itens de moda: confira se o título é mesmo o produto pedido.",
        ),
        site(
            "magalu", "Magazine Luiza", "magazineluiza.com.br",
            "https://www.magazineluiza.com.br/busca/{q_plus}/", "novos", false, None,
            "Marketplace de novos. Registre o preço à vista/PIX.",
        ),
        site(
            "shopee", "Shopee", "shopee.com.br",
            "https://shopee.com.br/search?keyword={q}", "novos", true,
            Some("https://shopee.com.br/buyer/login"),
            "Costuma exigir login para buscar. Muitos anúncios de peças/capas: filtre com cuidado.",
        ),
    ]
}

pub fn default_enabled() -> Vec<String> {
    ["mercadolivre", "olx", "amazon", "kabum", "terabyte", "pichau", "enjoei"]
        .into_iter()
        .map(String::from)
        .collect()
}
