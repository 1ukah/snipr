//! Condições de pagamento e ofertas lidas sem IA do texto de um card ou de
//! uma página de anúncio: parcelas, preço no PIX, cupons e cashback. As
//! regras valem para qualquer loja: só dependem de como as lojas brasileiras
//! escrevem essas condições ("10x de R$ 99,90 sem juros", "R$ 899 no Pix",
//! "Cupom BORA5", "5% OFF").

use std::sync::LazyLock;

use regex::Regex;

use crate::{parse::parse_brl, types::Listing};

const MONEY: &str = r"R\$\s*\d{1,3}(?:\.\d{3})*(?:,\d{2})?|R\$\s*\d+(?:,\d{2})?";

/// "10x de R$ 330,00 sem juros", "10x sem juros de R$ 330,00", "12 vezes R$ 99".
static INSTALLMENT: LazyLock<Regex> = LazyLock::new(|| {
    // Com "sem juros" no meio, o "de" é obrigatório: "10x sem juros R$ 3.300" cita o total, não a parcela.
    Regex::new(&format!(r"(?i)\b(\d{{1,2}})\s*(?:x|vezes)\s*(?:(sem juros|s/\s*juros|sem acr[eé]scimo|com juros)\s*de\s*|de\s*)?({MONEY})")).unwrap()
});
/// "Até 10x sem juros", sem o valor da parcela.
static UP_TO: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b(?:at[eé]|em)\s*(\d{1,2})\s*(?:x|vezes)\s*(sem juros|s/\s*juros|sem acr[eé]scimo)").unwrap());
/// "R$ 3.300 em até 10x": o valor antes é o total no cartão.
static TOTAL_IN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!(r"(?i)({MONEY})\s*(?:em at[eé]|at[eé]|em|ou)\s*(\d{{1,2}})\s*(?:x|vezes)")).unwrap());
static FREE_AFTER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^\s*(sem juros|s/\s*juros|sem acr[eé]scimo)").unwrap());
static PAID_AFTER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^\s*com juros").unwrap());
/// "R$ 3.201 no Pix", "R$ 1.299,99 À vista no PIX", "R$ 899 à vista".
static PIX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(r"(?i)({MONEY})\s*(?:(?:[aà] vista\s*)?(?:no|via|com|pelo|pagando no|pagando com)\s*(?:o\s*)?pix\b|[aà] vista)")).unwrap()
});
/// "Pix: R$ 899", "no Pix por R$ 899".
static PIX_BEFORE: LazyLock<Regex> = LazyLock::new(|| Regex::new(&format!(r"(?i)\bpix\s*(?::|por|de)\s*({MONEY})")).unwrap());
static PIX_OFF: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)(\d{1,2})\s*%\s*(?:off|de desconto|desconto)\s*(?:no|via|pagando no|pagando com)\s*pix\b").unwrap());
/// Código de cupom em maiúsculas depois de "cupom", "código", "digite"…
static CODE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"\b(?i:cupom|c[oó]digo|digite|use)\s*:?\s*["“'«]?([A-Z][A-Z0-9]{3,19})\b"#).unwrap());
static DISCOUNT: LazyLock<Regex> = LazyLock::new(|| Regex::new(&format!(r"(?i)(\d{{1,2}}\s*%|{MONEY})\s*(?:off\b|de desconto)")).unwrap());
/// "Cupom 10% OFF", "Cupom de R$ 20", "5% OFF com cupom".
static COUPON: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(r"(?i)cupom\s*(?:de\s*)?(\d{{1,2}}\s*%|{MONEY})|(\d{{1,2}}\s*%|{MONEY})\s*(?:off\s*)?(?:com|no|usando o|usando|via)\s*cupom")).unwrap()
});
static CASHBACK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(r"(?i)(\d{{1,2}}\s*%|{MONEY})\s*(?:de\s*|em\s*)?cashback|cashback\s*(?:de\s*)?(\d{{1,2}}\s*%|{MONEY})")).unwrap()
});
/// Na página de um anúncio, o texto depois destes títulos é de outros anúncios.
static OTHERS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)an[úu]ncios relacionados|produtos relacionados|voc[êe] tamb[ée]m pode gostar|quem viu este|veja tamb[ée]m|mais an[úu]ncios|compre junto|produtos similares|outros an[úu]ncios").unwrap()
});
/// Palavras em maiúsculas que não são código de cupom.
const NOT_CODE: &[&str] = &["CUPOM", "CUPONS", "DESCONTO", "LIMITADO", "LIMITADOS", "OFF", "FRETE", "GRATIS", "APLICADO", "EXCLUSIVO", "PIX", "AQUI", "PARA", "COPIAR", "VALIDO"];

#[derive(Debug, Default, Clone, PartialEq)]
pub struct Offer {
    /// Preço à vista ou no PIX.
    pub pix: Option<f64>,
    /// Total no cartão.
    pub card_total: Option<f64>,
    pub installments: Option<u32>,
    pub interest_free: bool,
    /// Cupons, cashback e descontos no PIX, prontos para mostrar.
    pub notes: Vec<String>,
}

fn money(s: &str) -> Option<f64> {
    parse_brl(s).filter(|v| *v > 0.0)
}

fn short(v: &str) -> String {
    let v = v.split_whitespace().collect::<Vec<_>>().join(" ");
    match money(&v) {
        Some(m) if v.starts_with("R$") => format!("R$ {}", m.round() as i64),
        _ => v.replace(' ', ""),
    }
}

/// Lê as condições de um texto. `page`: texto de uma página de anúncio (corta
/// as listas de outros anúncios).
pub fn read(text: &str, page: bool) -> Offer {
    let text = if page { OTHERS.find(text).map_or(text, |m| &text[..m.start()]) } else { text };
    let mut o = Offer::default();

    // Parcelas: o maior número sem juros; sem nenhuma sem juros, o maior.
    let mut best: Option<(u32, Option<f64>, bool)> = None;
    let mut consider = |n: u32, value: Option<f64>, free: bool| {
        if !(2..=24).contains(&n) {
            return;
        }
        let better = match best {
            None => true,
            Some((bn, bv, bf)) => (free, n, value.is_some()) > (bf, bn, bv.is_some()),
        };
        if better {
            best = Some((n, value, free));
        }
    };
    for c in INSTALLMENT.captures_iter(text) {
        let n: u32 = c[1].parse().unwrap_or(0);
        let after = &text[c.get(0).unwrap().end()..];
        let tag = c.get(2).map(|m| m.as_str().to_lowercase()).unwrap_or_default();
        let free = (!tag.is_empty() && !tag.starts_with("com")) || (tag.is_empty() && FREE_AFTER.is_match(after) && !PAID_AFTER.is_match(after));
        consider(n, money(&c[3]), free);
    }
    for c in UP_TO.captures_iter(text) {
        consider(c[1].parse().unwrap_or(0), None, true);
    }
    if let Some((n, value, free)) = best {
        o.installments = Some(n);
        o.interest_free = free;
        o.card_total = TOTAL_IN
            .captures_iter(text)
            .find(|c| c[2].parse::<u32>().ok() == Some(n))
            .and_then(|c| money(&c[1]))
            .or(value.map(|v| (v * n as f64 * 100.0).round() / 100.0));
    }

    o.pix = PIX.captures(text).and_then(|c| money(&c[1])).or_else(|| PIX_BEFORE.captures(text).and_then(|c| money(&c[1])));
    if let Some(c) = PIX_OFF.captures(text) {
        o.notes.push(format!("{}% OFF no PIX", &c[1]));
    }

    let push = |s: String, o: &mut Offer| {
        if !o.notes.iter().any(|n| n.eq_ignore_ascii_case(&s)) && o.notes.len() < 3 {
            o.notes.push(s);
        }
    };
    let mut coded = false;
    for c in CODE.captures_iter(text) {
        let code = c.get(1).unwrap();
        if NOT_CODE.contains(&code.as_str()) || code.as_str().len() < 4 {
            continue;
        }
        // O desconto costuma estar logo antes ou logo depois do código.
        let lo = text[..code.start()].char_indices().rev().nth(80).map_or(0, |(i, _)| i);
        let hi = text[code.end()..].char_indices().nth(80).map_or(text.len(), |(i, _)| code.end() + i);
        let near = DISCOUNT.captures(&text[lo..hi]).map(|d| format!(" ({} OFF)", short(&d[1]))).unwrap_or_default();
        push(format!("Cupom {}{near}", code.as_str()), &mut o);
        coded = true;
    }
    if !coded {
        if let Some(c) = COUPON.captures(text) {
            let v = c.get(1).or(c.get(2)).map(|m| short(m.as_str())).unwrap_or_default();
            push(format!("Cupom {v} OFF"), &mut o);
        }
    }
    if let Some(c) = CASHBACK.captures(text) {
        let v = c.get(1).or(c.get(2)).map(|m| short(m.as_str())).unwrap_or_default();
        push(format!("Cashback {v}"), &mut o);
    }
    o
}

/// Aplica as condições ao anúncio. Valores fora de proporção com o preço
/// (ex.: parcela de outro produto da página) ficam de fora. Devolve se algo
/// mudou.
pub fn apply(l: &mut Listing, o: &Offer) -> bool {
    let before = (l.price, l.card_price, l.installments, l.interest_free, l.offers.len());
    let shown = l.price;
    if let Some(pix) = o.pix.filter(|p| *p < shown && *p >= shown * 0.6) {
        l.price = pix;
        if l.card_price.is_none() {
            l.card_price = Some(shown);
        }
    }
    if let Some(n) = o.installments {
        let total = o.card_total.or(if o.interest_free { Some(l.card_price.unwrap_or(shown)) } else { None });
        if let Some(t) = total.filter(|t| *t >= l.price * 0.95 && *t <= l.price * 2.5) {
            l.card_price = Some(t);
            l.installments = Some(n);
            l.interest_free = o.interest_free;
        }
    }
    for n in &o.notes {
        if !l.offers.contains(n) && l.offers.len() < 4 {
            l.offers.push(n.clone());
        }
    }
    before != (l.price, l.card_price, l.installments, l.interest_free, l.offers.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn card_da_olx() {
        let o = read("VENDO PLAYSTATION 5 Frete grátis Até 10x sem juros R$ 3.300em até 10x de R$ 330,00 sem juros Feira de Santana", false);
        assert_eq!(o.installments, Some(10));
        assert!(o.interest_free);
        assert_eq!(o.card_total, Some(3300.0));
        assert_eq!(o.pix, None);
    }

    #[test]
    fn pagina_da_olx_com_cupom() {
        let o = read(
            "Feira de Santana, BA R$ 3.300 R$ 3.201 no Pix 10x sem juros de R$ 330,00 Mais opções de parcelamento 5% OFF Copie ou digite BORA5 na etapa de pagamento. *Cupons limitados. Copiar",
            true,
        );
        assert_eq!(o.pix, Some(3201.0));
        assert_eq!((o.installments, o.interest_free, o.card_total), (Some(10), true, Some(3300.0)));
        assert_eq!(o.notes, vec!["Cupom BORA5 (5% OFF)"]);
    }

    #[test]
    fn lojas_de_novos() {
        let k = read("R$ 1.529,40 R$ 1.299,99 À vista no PIX ou até 10x de R$ 152,94 sem juros", false);
        assert_eq!(k.pix, Some(1299.99));
        assert_eq!((k.installments, k.card_total), (Some(10), Some(1529.4)));
        let a = read("R$ 1.299,00 em até 12x R$ 108,25 com juros Economize 5% com cupom", false);
        assert_eq!((a.installments, a.interest_free), (Some(12), false));
        assert_eq!(a.notes, vec!["Cupom 5% OFF"]);
        let m = read("R$ 2.000 Cupom R$ 30 OFF 5% de cashback", false);
        assert_eq!(m.notes, vec!["Cupom R$ 30 OFF", "Cashback 5%"]);
        assert!(read("Cupom de desconto. Cupons limitados", false).notes.is_empty());
    }

    #[test]
    fn corta_outros_anuncios_da_pagina() {
        let o = read("R$ 500 Anúncios relacionados Outro produto 10x de R$ 90,00 sem juros", true);
        assert_eq!(o.installments, None);
    }

    /// Lê uma página real (internet, sem IA):
    ///   SNIPR_TEST_URL=<anúncio> cargo test -p snipr-core le_pagina_real -- --ignored --nocapture
    #[tokio::test]
    #[ignore]
    async fn le_pagina_real() {
        let url = std::env::var("SNIPR_TEST_URL").expect("SNIPR_TEST_URL");
        let page = crate::fetch::Fetcher::new().unwrap().get(crate::sites::Stage::B, &url, None).await.unwrap();
        let text = crate::parse::text_from_html(&page.body, 12_000);
        println!("HTTP {} · {} caracteres\n{:?}", page.status, text.len(), read(&text, true));
    }

    #[test]
    fn aplica_no_anuncio() {
        let mut l: Listing = serde_json::from_value(serde_json::json!({
            "id": "1", "site_id": "olx", "site_name": "OLX", "title": "PS5", "price": 3300.0, "currency": "BRL",
            "condition": "used", "url": "https://x"
        }))
        .unwrap();
        let o = read("R$ 3.300 R$ 3.201 no Pix 10x sem juros de R$ 330,00 5% OFF digite BORA5", true);
        assert!(apply(&mut l, &o));
        assert_eq!((l.price, l.card_price, l.installments), (3201.0, Some(3300.0), Some(10)));
        assert!(!apply(&mut l, &o), "a segunda vez não muda nada");
        // Parcela de outro produto: fora de proporção.
        let mut cheap = l.clone();
        cheap.card_price = None;
        cheap.installments = None;
        apply(&mut cheap, &read("12x de R$ 2.000,00 sem juros", false));
        assert_eq!(cheap.installments, None);
    }
}
