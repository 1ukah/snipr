//! Leitura de cards a partir de HTML ou JSON baixados por HTTP (etapas A, B e C).
//!
//! O HTML segue a mesma heurística do `EXTRACT_LISTINGS` que roda no Chrome:
//! parte de cada preço em reais e sobe até o menor ancestral com um link de
//! produto. Sem layout, não há como saber o que está visível; a verificação
//! da loja compara o resultado com o que o Chrome lê antes de aceitar a etapa.

use std::{collections::HashSet, sync::LazyLock};

use ego_tree::NodeRef;
use regex::Regex;
use scraper::{Html, Node, Selector};
use serde_json::Value;
use snipr_browser::Card;
use url::Url;

static PRICE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"R\$\s*\d{1,3}(?:\.\d{3})*(?:,\d{2})?|R\$\s*\d+(?:,\d{2})?").unwrap());
static HAS_PRICE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"R\$\s*\d").unwrap());
static TRACK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)click|clicks|clk|publicidade|adclick|redirect|/count\b|doubleclick|taboola|outbrain|criteo|rtb-|/ads?/").unwrap()
});
static JUNK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(imagem|foto|image|picture|pre[çc]o|price|parte (de tr[áa]s|da frente)|frente|verso|front|back|an[úu]ncio|patrocinado|sponsored|adicionar|comprar|ver mais|ver detalhes|favoritar|frete|entrega|em at[ée]|at[ée] \d|\d+\s*x\b|ou \d|sem juros|no pix|[àa] vista|mais op[çc][õo]es|off\b|oferta (black|rel[âa]mpago|do dia)|\(?\d+ ofertas|compre produtos de marcas)").unwrap()
});
static STRONG_PATH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\d{5,}|/dp/|/p/|produto|item|MLB").unwrap());
static WEAK_PATH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^/@|perfil|profile|seller|vendedor|/loja/|/store/|/user|/b$|/s$|/deals|/busca|/search").unwrap()
});
static INSTALLMENT_BEFORE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(\d+\s*x\s*(de\s*)?|parcelas?\s*(de\s*)?|mensais\s*(de\s*)?)$").unwrap());
static DISCOUNT_BEFORE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(economi[a-z]*|desconto( de)?|off|cashback|frete( de)?|ganhe|cupom( de)?|menos|poupe)\s*:?\s*$").unwrap());
static DISCOUNT_AFTER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*(off\b|de desconto|em desconto|de cashback|em cashback|de frete|em frete)").unwrap());
static OLD_BEFORE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(^|[\s(])de:?\s*$").unwrap());
static FROM_BEFORE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"a partir de\s*$").unwrap());
static INTL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\binternaciona(l|is)\b|\bimportad[oa]s?\b|envio do exterior|enviado do exterior|(envio|enviado|enviada|chega|despachado) (de|da|do|dos) (china|eua|estados unidos|hong kong|exterior)|\binternational\b").unwrap()
});
static OLD_CLASS: &[&str] = &[
    "old-price", "old_price", "oldprice", "price-old", "price_old", "priceold", "previous-price", "price-before", "original-price",
    "price-original", "originalprice", "list-price", "listprice",
];

const SKIP: &[&str] = &["script", "style", "noscript", "template", "svg", "head", "iframe"];
const BLOCK: &[&str] = &[
    "div", "p", "li", "ul", "ol", "tr", "td", "th", "table", "section", "article", "header", "footer", "h1", "h2", "h3", "h4", "h5", "h6",
    "br", "dd", "dt", "dl", "main", "aside", "nav", "form", "figure", "figcaption",
];

fn sel(s: &str) -> Selector {
    Selector::parse(s).expect("seletor")
}

fn clean(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Normaliza espaços e o formato dos preços ("R$1.299 , 90" → "R$ 1.299,90").
fn norm(s: &str) -> String {
    static CURRENCY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"R\$\s*").unwrap());
    static CENTS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(\d)\s*,\s*(\d{2})").unwrap());
    let s = clean(s);
    let s = CURRENCY.replace_all(&s, "R$ ");
    CENTS.replace_all(&s, "$1,$2").into_owned()
}

fn collect(node: NodeRef<Node>, lines: bool, out: &mut String) {
    for child in node.children() {
        match child.value() {
            Node::Text(t) => {
                out.push_str(t);
                out.push(' ');
            }
            Node::Element(e) => {
                if SKIP.contains(&e.name()) {
                    continue;
                }
                collect(child, lines, out);
                if lines && BLOCK.contains(&e.name()) {
                    out.push('\n');
                }
            }
            _ => {}
        }
    }
}

fn text_of(node: NodeRef<Node>) -> String {
    let mut s = String::new();
    collect(node, false, &mut s);
    s
}

fn tag(node: NodeRef<'_, Node>) -> &str {
    node.value().as_element().map(|e| e.name()).unwrap_or_default()
}

/// Separa os preços pelo contexto: parcelas, descontos/fretes e "de R$ X por
/// R$ Y" não são o preço atual. Mesmo critério do script do Chrome.
fn prices_of(text: &str) -> (Vec<String>, Vec<String>) {
    let (mut now, mut old, mut skip) = (Vec::<String>::new(), Vec::<String>::new(), HashSet::<String>::new());
    for m in PRICE.find_iter(text) {
        let p = clean(m.as_str());
        if skip.contains(&p) {
            continue;
        }
        let before: String = text[..m.start()].chars().rev().take(24).collect::<Vec<_>>().into_iter().rev().collect::<String>().to_lowercase();
        let after: String = text[m.end()..].chars().take(20).collect::<String>().to_lowercase();
        if INSTALLMENT_BEFORE.is_match(&before) || DISCOUNT_BEFORE.is_match(&before) || DISCOUNT_AFTER.is_match(&after) {
            skip.insert(p);
            continue;
        }
        if OLD_BEFORE.is_match(&before) && !FROM_BEFORE.is_match(&before) {
            if !old.contains(&p) {
                old.push(p);
            }
            continue;
        }
        if !now.contains(&p) {
            now.push(p);
        }
    }
    now.retain(|p| !old.contains(p) && !skip.contains(p));
    (now, old)
}

struct Ctx<'a> {
    base: &'a Url,
    here: String,
}

impl Ctx<'_> {
    fn product_url(&self, a: NodeRef<Node>) -> Option<Url> {
        let href = a.value().as_element()?.attr("href")?;
        let u = self.base.join(href).ok()?;
        if !matches!(u.scheme(), "http" | "https") {
            return None;
        }
        if TRACK.is_match(&format!("{}{}", u.host_str().unwrap_or_default(), u.path())) {
            return None;
        }
        if format!("{}{}", u.origin().ascii_serialization(), u.path()) == self.here || u.path().len() < 2 {
            return None;
        }
        Some(u)
    }

    fn link_score(&self, a: NodeRef<Node>, u: &Url) -> i64 {
        let path = u.path();
        let rich = a.descendants().any(|d| matches!(tag(d), "img" | "h1" | "h2" | "h3" | "h4"));
        path.len() as i64 + if STRONG_PATH.is_match(path) { 60 } else { 0 } + if rich { 30 } else { 0 }
            - if path.len() < 12 { 80 } else { 0 }
            - if WEAK_PATH.is_match(path) { 150 } else { 0 }
    }

    fn anchors<'n>(&self, b: NodeRef<'n, Node>) -> Vec<(NodeRef<'n, Node>, Url)> {
        let nodes: Vec<NodeRef<Node>> = if tag(b) == "a" { vec![b] } else { b.descendants().filter(|d| tag(*d) == "a").collect() };
        nodes.into_iter().filter_map(|a| self.product_url(a).map(|u| (a, u))).collect()
    }

    fn has_title(&self, b: NodeRef<Node>) -> bool {
        b.descendants().any(|e| {
            let Some(el) = e.value().as_element() else { return false };
            let t = match el.name() {
                "a" => match self.product_url(e) {
                    Some(u) if self.link_score(e, &u) >= 40 => clean(&text_of(e)),
                    _ => return false,
                },
                "h1" | "h2" | "h3" | "h4" => clean(&text_of(e)),
                _ => clean(el.attr("title").unwrap_or_default()),
            };
            t.chars().count() >= 15 && !HAS_PRICE.is_match(&t) && !JUNK.is_match(&t)
        })
    }
}

/// Identificador do produto na URL (ASIN, MLB…, código da loja).
fn product_key(u: &Url) -> String {
    u.path()
        .split(['/', '_', '.', '-'])
        .filter(|p| p.len() >= 5 && p.bytes().any(|b| b.is_ascii_digit()))
        .max_by_key(|p| p.len())
        .map(String::from)
        .unwrap_or_else(|| u.path().to_string())
}

fn is_old_price_el(e: NodeRef<Node>) -> bool {
    let Some(el) = e.value().as_element() else { return false };
    if matches!(el.name(), "s" | "del") {
        return true;
    }
    let class = el.attr("class").unwrap_or_default().to_lowercase();
    OLD_CLASS.iter().any(|c| class.contains(c)) || el.attr("data-testid").is_some_and(|t| t.to_lowercase().contains("original"))
}

fn card_url(u: &Url) -> String {
    let mut url = format!("{}{}", u.origin().ascii_serialization(), u.path());
    if u.path().len() < 25 {
        if let Some(q) = u.query() {
            url.push('?');
            url.push_str(&q.chars().take(120).collect::<String>());
        }
    }
    url
}

fn is_intl(b: NodeRef<Node>, text: &str) -> bool {
    let labels: Vec<&str> = b
        .descendants()
        .filter_map(|d| d.value().as_element())
        .filter_map(|e| e.attr("alt").or_else(|| e.attr("aria-label")).or_else(|| e.attr("title")))
        .collect();
    INTL.is_match(&format!("{text} {}", labels.join(" ")))
}

/// Cards de uma página de resultados: HTML, JSON-LD e JSON embutido
/// (ex.: `__NEXT_DATA__`). `json_url` monta o link de produtos do JSON que
/// não trazem URL (ex.: "https://loja/produto/{code}/{slug}").
pub fn cards_from_html(html: &str, page_url: &str, max: usize, json_url: Option<&str>) -> Vec<Card> {
    let Ok(base) = Url::parse(page_url) else { return Vec::new() };
    let doc = Html::parse_document(html);
    let ctx = Ctx { here: format!("{}{}", base.origin().ascii_serialization(), base.path()), base: &base };
    let mut cards = dom_cards(&doc, &ctx, max);
    let mut known: HashSet<String> = cards.iter().map(|c| c.url.clone()).collect();
    let extra = jsonld_cards(&doc, &base).into_iter().chain(embedded_cards(&doc, &base, json_url));
    for c in extra {
        if cards.len() >= max {
            break;
        }
        if known.insert(c.url.clone()) {
            cards.push(c);
        }
    }
    cards
}

const NAME_KEYS: &[&str] = &["name", "title", "productName", "product_name", "nome"];
/// Do mais para o menos provável de ser o preço à vista.
const PRICE_KEYS: &[&str] = &[
    "priceWithDiscount", "pixPrice", "spotPrice", "cashPrice", "salePrice", "sellingPrice", "bestPrice", "finalPrice", "price", "Price",
    "preco", "precoPor", "valor",
];
const OLD_KEYS: &[&str] = &["oldPrice", "listPrice", "ListPrice", "priceFrom", "precoDe", "originalPrice", "regularPrice", "compareAtPrice"];
const URL_KEYS: &[&str] = &["url", "link", "href", "permalink", "productUrl", "product_url"];

/// Objetos com cara de produto (nome + preço) nos JSON embutidos da página.
fn embedded_products(doc: &Html) -> Vec<serde_json::Map<String, Value>> {
    let mut out = Vec::new();
    for s in doc.select(&sel(r#"script[type="application/json"], script#__NEXT_DATA__"#)) {
        let Ok(v) = serde_json::from_str::<Value>(&s.text().collect::<String>()) else { continue };
        find_products(&v, &mut out, 0);
    }
    out
}

fn find_products(v: &Value, out: &mut Vec<serde_json::Map<String, Value>>, depth: usize) {
    if depth > 14 || out.len() > 300 {
        return;
    }
    match v {
        Value::Array(a) => a.iter().for_each(|x| find_products(x, out, depth + 1)),
        Value::Object(o) => {
            let named = NAME_KEYS.iter().any(|k| o.get(*k).and_then(Value::as_str).is_some_and(|s| s.trim().chars().count() >= 6));
            let priced = PRICE_KEYS.iter().any(|k| o.get(*k).and_then(number).is_some_and(|p| p > 0.0));
            if named && priced {
                out.push(o.clone());
                return;
            }
            o.values().for_each(|x| find_products(x, out, depth + 1));
        }
        // JSON serializado dentro do JSON (ex.: `pageProps.data` em lojas Next.js).
        Value::String(s) if s.len() > 200 && (s.starts_with('{') || s.starts_with('[')) => {
            if let Ok(inner) = serde_json::from_str::<Value>(s) {
                find_products(&inner, out, depth + 1);
            }
        }
        _ => {}
    }
}

fn scalar(v: &Value) -> Option<String> {
    match v {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn fill_template(template: &str, o: &serde_json::Map<String, Value>) -> Option<String> {
    let mut out = template.to_string();
    while let Some(start) = out.find('{') {
        let end = start + out[start..].find('}')?;
        let value = scalar(o.get(&out[start + 1..end])?)?;
        out.replace_range(start..=end, &value);
    }
    Some(out)
}

fn embedded_cards(doc: &Html, base: &Url, json_url: Option<&str>) -> Vec<Card> {
    let mut out = Vec::new();
    for o in embedded_products(doc) {
        let unavailable = matches!(o.get("available"), Some(Value::Bool(false))) || o.get("available").and_then(Value::as_str).is_some_and(|s| s.eq_ignore_ascii_case("false"));
        if unavailable {
            continue;
        }
        let title = NAME_KEYS.iter().find_map(|k| o.get(*k).and_then(Value::as_str)).map(clean).unwrap_or_default();
        let link = URL_KEYS
            .iter()
            .find_map(|k| o.get(*k).and_then(Value::as_str).filter(|u| u.starts_with('/') || u.starts_with("http")).map(String::from))
            .or_else(|| json_url.and_then(|t| fill_template(t, &o)));
        let Some(u) = link.and_then(|l| base.join(&l).ok()) else { continue };
        let mut prices: Vec<f64> = PRICE_KEYS.iter().filter_map(|k| o.get(*k).and_then(number)).filter(|p| *p > 0.0).collect();
        prices.dedup();
        let top = prices.iter().copied().fold(0.0, f64::max);
        let old: Vec<String> = OLD_KEYS.iter().filter_map(|k| o.get(*k).and_then(number)).filter(|p| *p > top).map(format_brl).take(1).collect();
        if prices.is_empty() {
            continue;
        }
        out.push(Card { title, prices: prices.into_iter().map(format_brl).collect(), old, url: card_url(&u), extra: String::new(), intl: false });
    }
    out
}

/// Aprende como montar o link dos produtos do JSON embutido comparando com
/// os links que o Chrome leu (ex.: "/produto/123/nome" ← campos code e slug).
pub fn learn_json_url(html: &str, reference: &[Card]) -> Option<String> {
    let doc = Html::parse_document(html);
    // Título inteiro: variações do mesmo produto começam igual.
    let full = |t: &str| fold(&clean(t)).chars().take(120).collect::<String>();
    let by_title: std::collections::HashMap<String, &Card> = reference.iter().map(|c| (full(&c.title), c)).collect();
    let mut votes: std::collections::HashMap<String, usize> = Default::default();
    let mut pairs = 0;
    for o in embedded_products(&doc) {
        let Some(name) = NAME_KEYS.iter().find_map(|k| o.get(*k).and_then(Value::as_str)) else { continue };
        let Some(card) = by_title.get(&full(name)) else { continue };
        let Ok(u) = Url::parse(&card.url) else { continue };
        let mut hit = false;
        let path: Vec<String> = u
            .path()
            .split('/')
            .map(|seg| match o.iter().find(|(_, v)| scalar(v).is_some_and(|s| s.len() >= 3 && s == seg)) {
                Some((k, _)) => {
                    hit = true;
                    format!("{{{k}}}")
                }
                None => seg.to_string(),
            })
            .collect();
        if hit {
            pairs += 1;
            *votes.entry(format!("{}{}", u.origin().ascii_serialization(), path.join("/"))).or_default() += 1;
        }
    }
    tracing::debug!(pairs, ?votes, "modelo de link do JSON");
    let (template, n) = votes.into_iter().max_by_key(|(_, n)| *n)?;
    (n >= 2 && n * 10 >= pairs * 8).then_some(template)
}

fn dom_cards(doc: &Html, ctx: &Ctx, max: usize) -> Vec<Card> {
    let body = doc.select(&sel("body")).next().map(|b| *b).unwrap_or_else(|| doc.tree.root());
    // 1) Menores elementos cujo texto contém um preço.
    let mut price_els: Vec<NodeRef<Node>> = Vec::new();
    let mut seen = HashSet::new();
    for n in body.descendants() {
        let Node::Text(t) = n.value() else { continue };
        if !t.contains("R$") {
            continue;
        }
        let mut el = n.parent();
        while let Some(e) = el {
            if e.value().as_element().is_none() || SKIP.contains(&tag(e)) {
                el = None;
                break;
            }
            let txt = norm(&text_of(e));
            if txt.chars().count() > 60 {
                el = None;
                break;
            }
            if PRICE.is_match(&txt) {
                break;
            }
            el = e.parent();
        }
        if let Some(e) = el {
            if e.ancestors().any(|a| SKIP.contains(&tag(a))) {
                continue;
            }
            if seen.insert(e.id()) {
                price_els.push(e);
            }
        }
    }

    // 2) De cada preço, sobe até o menor ancestral com link de produto.
    let mut cards = Vec::new();
    let (mut boxes, mut urls) = (HashSet::new(), HashSet::new());
    for p in price_els {
        if cards.len() >= max {
            break;
        }
        let mut cur = Some(p);
        let mut prev: Option<NodeRef<Node>> = None;
        let mut link: Option<(NodeRef<Node>, Url)> = None;
        let (mut extra_up, mut title_up) = (0, 0);
        for _ in 0..12 {
            let Some(b) = cur else { break };
            if b.id() == body.id() {
                break;
            }
            let mut prods = ctx.anchors(b);
            if !prods.is_empty() {
                let distinct: HashSet<&str> = prods.iter().map(|(_, u)| u.path()).collect();
                let strong: HashSet<String> =
                    prods.iter().filter(|(a, u)| ctx.link_score(*a, u) >= 40).map(|(_, u)| product_key(u)).collect();
                if distinct.len() > 3 || (title_up > 0 && strong.len() > 1) {
                    cur = if link.is_some() { prev } else { None };
                    break;
                }
                prods.sort_by_key(|(a, u)| -ctx.link_score(*a, u));
                let best = prods.swap_remove(0);
                let weak = ctx.link_score(best.0, &best.1) < 40;
                link = Some(best);
                let parent = b.parent().filter(|x| x.value().is_element() && x.id() != body.id());
                if weak && extra_up < 2 && text_of(b).len() < 800 {
                    extra_up += 1;
                    prev = Some(b);
                    cur = parent;
                    continue;
                }
                if !ctx.has_title(b) && title_up < 4 && parent.is_some() {
                    title_up += 1;
                    prev = Some(b);
                    cur = parent;
                    continue;
                }
                break;
            }
            cur = b.parent().filter(|x| x.value().is_element());
        }
        let (Some(b), Some((link_a, u))) = (cur, link) else { continue };
        if !boxes.insert(b.id()) {
            continue;
        }
        let raw = text_of(b);
        if clean(&raw).chars().count() > 1500 {
            continue;
        }
        let url = card_url(&u);
        if urls.contains(&url) {
            continue;
        }
        let text = norm(&raw);

        // Título: atributos title/alt e headings valem mais; depois o texto mais longo sem preço.
        let (mut title, mut best) = (String::new(), -1i64);
        for x in std::iter::once(link_a).chain(b.descendants()) {
            let Some(el) = x.value().as_element() else { continue };
            if SKIP.contains(&el.name()) || x.ancestors().any(|a| SKIP.contains(&tag(a))) {
                continue;
            }
            let mut t = clean(el.attr("title").or(if el.name() == "img" { el.attr("alt") } else { None }).unwrap_or_default());
            let mut bonus = 30;
            if JUNK.is_match(&t) {
                t.clear();
            }
            if t.is_empty() && matches!(el.name(), "h1" | "h2" | "h3" | "h4" | "h5" | "h6") {
                t = clean(&text_of(x));
                bonus = 60;
            }
            if t.is_empty() && x.children().filter(|c| c.value().is_element()).count() <= 2 {
                t = clean(&text_of(x));
                bonus = 0;
            }
            let t = t.trim_start_matches("Anúncio patrocinado").trim_start_matches(['–', '-', ' ']).to_string();
            let len = t.chars().count();
            if !(8..=200).contains(&len) || HAS_PRICE.is_match(&t) || JUNK.is_match(&t) {
                continue;
            }
            if len as i64 + bonus > best {
                best = len as i64 + bonus;
                title = t;
            }
        }
        if title.is_empty() {
            continue;
        }
        let dom_old: Vec<String> = b
            .descendants()
            .filter(|e| is_old_price_el(*e))
            .map(|e| norm(&text_of(e)))
            .filter(|t| t.chars().count() <= 24 && PRICE.replace_all(t, "").chars().all(|c| !c.is_alphanumeric()))
            .filter_map(|t| PRICE.find(&t).map(|m| clean(m.as_str())))
            .collect();
        let (now, text_old) = prices_of(&text);
        let mut old: Vec<String> = Vec::new();
        for p in text_old.into_iter().chain(dom_old) {
            if !old.contains(&p) {
                old.push(p);
            }
        }
        old.truncate(2);
        let prices: Vec<String> = now.into_iter().filter(|p| !old.contains(p)).take(4).collect();
        // Sem preço, ou só R$ 0,00 (ex.: mini-carrinho do cabeçalho): não é anúncio.
        if prices.iter().all(|p| parse_brl(p).unwrap_or(0.0) <= 0.0) {
            continue;
        }
        urls.insert(url.clone());
        let extra: String = clean(&PRICE.replace_all(&text.replacen(&title, "", 1), " ")).chars().take(170).collect();
        let intl = is_intl(b, &text);
        cards.push(Card { title: title.chars().take(160).collect(), prices, old, url, extra, intl });
    }
    cards
}

/// Produtos declarados em JSON-LD (schema.org Product / ItemList).
fn jsonld_cards(doc: &Html, base: &Url) -> Vec<Card> {
    let mut out = Vec::new();
    for s in doc.select(&sel(r#"script[type="application/ld+json"]"#)) {
        let Ok(v) = serde_json::from_str::<Value>(&s.text().collect::<String>()) else { continue };
        walk_products(&v, base, &mut out, 0);
    }
    out
}

fn has_type(v: &Value, t: &str) -> bool {
    match &v["@type"] {
        Value::String(s) => s == t,
        Value::Array(a) => a.iter().any(|x| x == t),
        _ => false,
    }
}

fn number(v: &Value) -> Option<f64> {
    v.as_f64().or_else(|| v.as_str().and_then(|s| s.trim().replace(',', ".").parse().ok()))
}

fn walk_products(v: &Value, base: &Url, out: &mut Vec<Card>, depth: usize) {
    if depth > 8 {
        return;
    }
    match v {
        Value::Array(a) => a.iter().for_each(|x| walk_products(x, base, out, depth + 1)),
        Value::Object(o) => {
            if has_type(v, "Product") {
                let offers = match &v["offers"] {
                    Value::Array(a) => a.first().cloned().unwrap_or(Value::Null),
                    o => o.clone(),
                };
                let price = number(&offers["price"]).or_else(|| number(&offers["lowPrice"]));
                let name = v["name"].as_str().map(clean).unwrap_or_default();
                let url = v["url"].as_str().or(offers["url"].as_str()).and_then(|u| base.join(u).ok());
                let in_stock = !offers["availability"].as_str().is_some_and(|a| a.contains("OutOfStock"));
                if let (Some(p), Some(u)) = (price.filter(|p| *p > 0.0), url) {
                    if !name.is_empty() && in_stock {
                        out.push(Card { title: name, prices: vec![format_brl(p)], url: card_url(&u), ..Default::default() });
                    }
                }
                return;
            }
            for x in o.values() {
                walk_products(x, base, out, depth + 1);
            }
        }
        _ => {}
    }
}

/// Produtos da API de catálogo VTEX (`/api/catalog_system/pub/products/search`).
pub fn cards_from_vtex(json: &str, max: usize) -> Vec<Card> {
    let Ok(Value::Array(items)) = serde_json::from_str::<Value>(json) else { return Vec::new() };
    let mut out = Vec::new();
    for p in items {
        if out.len() >= max {
            break;
        }
        let title = p["productName"].as_str().map(clean).unwrap_or_default();
        let url = p["link"].as_str().unwrap_or_default().to_string();
        // Oferta mais barata entre os vendedores com estoque.
        let best = p["items"]
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|i| i["sellers"].as_array().cloned().unwrap_or_default())
            .map(|s| s["commertialOffer"].clone())
            .filter(|o| o["AvailableQuantity"].as_f64().unwrap_or(0.0) > 0.0)
            .filter_map(|o| Some((o["Price"].as_f64().filter(|p| *p > 0.0)?, o["ListPrice"].as_f64().unwrap_or(0.0))))
            .min_by(|a, b| a.0.total_cmp(&b.0));
        let Some((price, list)) = best else { continue };
        if title.is_empty() || !url.starts_with("http") {
            continue;
        }
        let old = if list > price { vec![format_brl(list)] } else { Vec::new() };
        let extra = p["brand"].as_str().unwrap_or_default().to_string();
        out.push(Card { title, prices: vec![format_brl(price)], old, url, extra, intl: false });
    }
    out
}

/// Texto de uma página de anúncio: dados do produto em JSON-LD (condição,
/// especificações) e o texto principal, sem menus nem scripts.
pub fn text_from_html(html: &str, max: usize) -> String {
    let doc = Html::parse_document(html);
    let mut out = String::new();
    for s in doc.select(&sel(r#"script[type="application/ld+json"]"#)) {
        let Ok(v) = serde_json::from_str::<Value>(&s.text().collect::<String>()) else { continue };
        product_facts(&v, &mut out, 0);
    }
    let main = ["main", "[role=main]", "#main", "#content", "body"].iter().find_map(|s| doc.select(&sel(s)).next());
    if let Some(m) = main {
        let mut raw = String::new();
        collect(*m, true, &mut raw);
        // Menus e rodapés dentro do <main> são raros; os de fora já ficaram de fora.
        for line in raw.lines().map(clean).filter(|l| !l.is_empty()) {
            out.push_str(&line);
            out.push('\n');
            if out.len() > max * 2 {
                break;
            }
        }
    }
    out.chars().take(max).collect()
}

fn product_facts(v: &Value, out: &mut String, depth: usize) {
    if depth > 6 {
        return;
    }
    match v {
        Value::Array(a) => a.iter().for_each(|x| product_facts(x, out, depth + 1)),
        Value::Object(o) if has_type(v, "Product") => {
            if let Some(n) = v["name"].as_str() {
                out.push_str(&format!("Produto: {}\n", clean(n)));
            }
            let offer = match &v["offers"] {
                Value::Array(a) => a.first().cloned().unwrap_or(Value::Null),
                x => x.clone(),
            };
            if let Some(c) = offer["itemCondition"].as_str().or(v["itemCondition"].as_str()) {
                let c = c.rsplit('/').next().unwrap_or(c);
                out.push_str(&format!("Condição (dados da loja): {c}\n"));
            }
            if let Some(d) = v["description"].as_str() {
                out.push_str(&format!("Descrição: {}\n", clean(d).chars().take(800).collect::<String>()));
            }
            for p in v["additionalProperty"].as_array().into_iter().flatten() {
                if let (Some(k), Some(val)) = (p["name"].as_str(), p["value"].as_str().map(String::from).or(p["value"].as_f64().map(|f| f.to_string()))) {
                    out.push_str(&format!("{k}: {val}\n"));
                }
            }
            let _ = o;
        }
        Value::Object(o) => o.values().for_each(|x| product_facts(x, out, depth + 1)),
        _ => {}
    }
}

/// "R$ 1.299,90" → 1299.9
pub fn parse_brl(s: &str) -> Option<f64> {
    let digits: String = s.chars().filter(|c| c.is_ascii_digit() || *c == ',' || *c == '.').collect();
    let n = if digits.contains(',') { digits.replace('.', "").replace(',', ".") } else { digits.replace('.', "") };
    n.parse().ok()
}

pub fn format_brl(v: f64) -> String {
    let cents = (v * 100.0).round() as i64;
    let (int, c) = (cents / 100, cents % 100);
    let s = int.to_string();
    let mut grouped = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            grouped.push('.');
        }
        grouped.push(ch);
    }
    format!("R$ {grouped},{c:02}")
}

/// Preços do card do menor para o maior. O primeiro é o padrão: nas lojas,
/// o menor preço sem parcela é o à vista (PIX).
pub fn sorted_prices(card: &Card) -> Vec<f64> {
    let mut v: Vec<f64> = card.prices.iter().filter_map(|p| parse_brl(p)).filter(|p| *p > 0.0).collect();
    v.sort_by(|a, b| a.total_cmp(b));
    v.dedup();
    v
}

/// Minúsculas sem acentos, para comparar textos.
pub fn fold(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .map(|c| match c {
            'á' | 'à' | 'â' | 'ã' | 'ä' => 'a',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'í' | 'ì' | 'î' | 'ï' => 'i',
            'ó' | 'ò' | 'ô' | 'õ' | 'ö' => 'o',
            'ú' | 'ù' | 'û' | 'ü' => 'u',
            'ç' => 'c',
            'ñ' => 'n',
            c => c,
        })
        .collect()
}

const STOP: &[&str] = &["com", "para", "sem", "ate", "uma", "por", "dos", "das", "nos", "nas", "que", "mais", "menos", "the", "and", "for"];

/// Palavras relevantes de um texto de busca (sem acentos, plural simples).
pub fn tokens(s: &str) -> Vec<String> {
    let mut out: Vec<String> = fold(s)
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() >= 3 && !STOP.contains(w))
        .map(|w| if w.len() > 4 { w.trim_end_matches('s').to_string() } else { w.to_string() })
        .collect();
    out.dedup();
    out
}

/// O card cita alguma das palavras.
pub fn mentions(card: &Card, words: &[String]) -> bool {
    if words.is_empty() {
        return true;
    }
    let hay = fold(&format!("{} {}", card.title, card.extra));
    words.iter().any(|w| hay.contains(w.as_str()))
}

/// Palavras do texto como aparecem num título compactado: "X-Pro" → "xpro".
fn keys(s: &str) -> Vec<String> {
    const VAGUE: &[&str] = &["modelo", "marca", "linha", "serie", "versao", "tipo", "cor"];
    let mut out: Vec<String> = Vec::new();
    for w in fold(s).split(|c: char| c.is_whitespace() || matches!(c, ',' | ';' | '/' | '(' | ')' | '+' | '|')) {
        let w: String = w.chars().filter(|c| c.is_alphanumeric()).collect();
        let digit = w.chars().any(|c| c.is_ascii_digit());
        if (w.len() >= 3 || (digit && w.len() >= 2)) && !STOP.contains(&w.as_str()) && !VAGUE.contains(&w.as_str()) && !out.contains(&w) {
            out.push(w);
        }
    }
    out
}

fn compact(s: &str) -> String {
    fold(s).chars().filter(|c| c.is_alphanumeric()).collect()
}

/// Palavras que distinguem cada alternativa do plano (sem as do tipo de
/// produto). Sem alternativas, cada termo de busca vira um grupo.
pub fn plan_groups(product: &str, alternatives: &[String], terms: &[String]) -> Vec<Vec<String>> {
    let generic: HashSet<String> = keys(product).into_iter().flat_map(|k| [k.trim_end_matches('s').to_string(), k]).collect();
    let source = if alternatives.is_empty() { terms } else { alternatives };
    let mut groups: Vec<Vec<String>> = Vec::new();
    for g in source.iter().map(|a| keys(a).into_iter().filter(|k| !generic.contains(k) && !generic.contains(k.trim_end_matches('s'))).collect::<Vec<_>>()) {
        if !g.is_empty() && !groups.contains(&g) {
            groups.push(g);
        }
    }
    groups
}

/// Escolhe até `max` cards de uma loja para o filtro da IA. Cada alternativa
/// do plano recebe sua vez (os cards da primeira busca não ocupam todas as
/// vagas) e só entram cards que citam a alternativa. Palavras que aparecem em
/// quase todos os cards da loja (ex.: a marca, na loja da marca) valem menos.
/// Sem grupos, vale a regra antiga: citar alguma palavra do plano.
///
/// `strict` (o plano tem alternativas, isto é, modelos): o card precisa citar
/// a maior parte da alternativa. Sem alternativas, os grupos vêm dos termos de
/// busca, que trazem especificações escritas de outro jeito nos títulos.
///
/// Quando boa parte dos cards da loja cita o tipo de produto ("cadeira"), um
/// card que não cita, e só cita uma palavra do modelo, é de outro tipo (ex.:
/// "Royce" traz carteiras, "XXL" traz camisetas). Se os títulos não usam a
/// palavra (ex.: "celular" × "Smartphone"), a regra não vale.
pub fn shortlist<'a>(cards: &[&'a Card], groups: &[Vec<String>], product: &str, strict: bool, words: &[String], max: usize) -> Vec<&'a Card> {
    if groups.is_empty() {
        return cards.iter().copied().filter(|c| mentions(c, words)).take(max).collect();
    }
    let all: Vec<String> = cards.iter().map(|c| compact(&format!("{} {}", c.title, c.extra))).collect();
    let head = keys(product).into_iter().next().map(|k| k.trim_end_matches('s').to_string());
    let typed = |h: &String| head.as_ref().is_none_or(|k| h.contains(k.as_str()));
    let common = all.iter().filter(|h| typed(h)).count() * 100 >= all.len() * 15;
    // Duas palavras do modelo bastam (sinônimos do tipo: "Smartphone" para "celular").
    let named = |h: &String| groups.iter().any(|g| g.iter().filter(|k| h.contains(k.as_str())).count() >= 2);
    let (cards, hay): (Vec<&'a Card>, Vec<String>) = cards.iter().copied().zip(all).filter(|(_, h)| !common || typed(h) || named(h)).unzip();
    let n = cards.len() as f64;
    let weight = |k: &str| {
        let df = hay.iter().filter(|h| h.contains(k)).count().max(1) as f64;
        (1.0 + n / df).ln()
    };
    let mut ranked: Vec<Vec<usize>> = Vec::new();
    for g in groups {
        let w: Vec<f64> = g.iter().map(|k| weight(k)).collect();
        let total: f64 = w.iter().sum();
        let mut scored: Vec<(usize, f64)> = hay
            .iter()
            .enumerate()
            .map(|(i, h)| (i, g.iter().zip(&w).filter(|(k, _)| h.contains(k.as_str())).map(|(_, w)| w).sum::<f64>() / total))
            .filter(|(_, s)| *s > 0.0)
            .collect();
        scored.sort_by(|a, b| b.1.total_cmp(&a.1));
        // Cards bem abaixo do melhor da alternativa são outro modelo (ex.: só a marca bate).
        let best = scored.first().map(|s| s.1).unwrap_or(0.0);
        let floor = if strict { 0.5f64.max(best * 0.5) } else { best * 0.5 };
        ranked.push(scored.into_iter().filter(|(_, s)| *s >= floor).map(|(i, _)| i).collect());
    }
    let (mut out, mut taken) = (Vec::new(), HashSet::new());
    let mut pos = vec![0usize; ranked.len()];
    while out.len() < max {
        let mut moved = false;
        for (g, list) in ranked.iter().enumerate() {
            while let Some(&i) = list.get(pos[g]) {
                pos[g] += 1;
                if taken.insert(i) {
                    out.push(cards[i]);
                    moved = true;
                    break;
                }
            }
            if out.len() >= max {
                break;
            }
        }
        if !moved {
            break;
        }
    }
    out
}

/// Busca mais curta para quando nenhum card do termo cita o modelo dele. Fica
/// só com as palavras raras nos resultados: em lojas que buscam qualquer
/// palavra, o resto casa com o catálogo todo ("cadeira XT Office X-Pro" na
/// loja da XT → "X-Pro").
pub fn narrower_term(term: &str, product: &str, cards: &[Card]) -> Option<String> {
    let group = plan_groups(product, &[], &[term.to_string()]).pop()?;
    let refs: Vec<&Card> = cards.iter().collect();
    if !shortlist(&refs, std::slice::from_ref(&group), product, true, &[], 1).is_empty() {
        return None;
    }
    let hay: Vec<String> = cards.iter().map(|c| compact(&format!("{} {}", c.title, c.extra))).collect();
    let common = (hay.len() as f64 * 0.3).max(1.0);
    let kept: Vec<&str> = term
        .split_whitespace()
        .filter(|w| {
            let k = compact(w);
            group.contains(&k) && (hay.iter().filter(|h| h.contains(&k)).count() as f64) < common
        })
        .collect();
    let q = kept.join(" ");
    (!q.is_empty() && !q.eq_ignore_ascii_case(term.trim())).then_some(q)
}

/// Fração dos cards que cita a busca.
pub fn relevance(cards: &[Card], query: &str) -> f64 {
    let words = tokens(query);
    if cards.is_empty() {
        return 0.0;
    }
    cards.iter().filter(|c| mentions(c, &words)).count() as f64 / cards.len() as f64
}

fn title_key(t: &str) -> String {
    fold(&clean(t)).chars().take(24).collect()
}

fn path_key(u: &str) -> Option<String> {
    let p = Url::parse(u).ok()?.path().trim_end_matches('/').to_lowercase();
    (p.len() >= 8).then_some(p)
}

/// Fração dos cards de referência (lidos no Chrome) que também aparecem em `got`.
pub fn coverage(reference: &[Card], got: &[Card]) -> f64 {
    if reference.is_empty() {
        return 0.0;
    }
    let paths: HashSet<String> = got.iter().filter_map(|c| path_key(&c.url)).collect();
    let titles: HashSet<String> = got.iter().map(|c| title_key(&c.title)).collect();
    let hit = reference
        .iter()
        .filter(|c| path_key(&c.url).is_some_and(|p| paths.contains(&p)) || titles.contains(&title_key(&c.title)))
        .count();
    hit as f64 / reference.len() as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: &str = r#"<html><body>
      <header><a href="/conta">Minha conta</a></header>
      <ul class="grid">
        <li class="card">
          <a href="/produto/123456/monitor-gamer-lg-27"><img alt="Imagem do produto"><h2>Monitor Gamer LG 27 IPS 144Hz</h2></a>
          <span class="price-old">R$ 1.499,90</span>
          <span class="font-semibold">R$ 1.099,90</span> <span>no Pix</span>
          <span>ou 10x de R$ 119,99 sem juros</span>
          <span>R$ 20,00 off na primeira compra</span>
        </li>
        <li class="card">
          <a href="/produto/654321/monitor-aoc-27"><h2>Monitor AOC 27 Full HD 180Hz</h2></a>
          <span>de R$ 999,00 por</span> <span>R$ 849,00</span>
          <span>Envio internacional</span>
        </li>
        <li class="card">
          <a href="/produto/777777/monitor-samsung-27"><h2>Monitor Samsung 27 Curvo 240Hz</h2></a>
          <span>R$ 1.899,00</span>
        </li>
        <li class="card">
          <a href="/produto/888888/monitor-philips-27"><h2>Monitor Philips 27 Gamer 120Hz</h2></a>
          <span>R$ 589,00</span>
        </li>
      </ul>
      <script>var x = "R$ 1,00";</script>
    </body></html>"#;

    #[test]
    fn le_cards_do_html() {
        let cards = cards_from_html(PAGE, "https://loja.com.br/busca?q=monitor", 40, None);
        assert_eq!(cards.len(), 4, "{cards:#?}");
        let lg = &cards[0];
        assert_eq!(lg.title, "Monitor Gamer LG 27 IPS 144Hz");
        assert_eq!(lg.prices, vec!["R$ 1.099,90"], "sem parcela, sem desconto, sem riscado");
        assert_eq!(lg.old, vec!["R$ 1.499,90"]);
        assert_eq!(lg.url, "https://loja.com.br/produto/123456/monitor-gamer-lg-27");
        let aoc = &cards[1];
        assert_eq!(aoc.prices, vec!["R$ 849,00"]);
        assert_eq!(aoc.old, vec!["R$ 999,00"], "\"de R$ X por\" é o preço antigo");
        assert!(aoc.intl);
        assert!(!cards[2].intl);
    }

    #[test]
    fn busca_mais_curta_quando_o_modelo_nao_aparece() {
        let card = |t: &str| Card { title: t.into(), ..Default::default() };
        let flood: Vec<Card> = (0..20).map(|i| card(&format!("Base Universal Para Cadeira Gamer E Office XT Racer {i}"))).collect();
        assert_eq!(narrower_term("cadeira XT Office X-Pro", "cadeira", &flood).as_deref(), Some("X-Pro"));
        let found = [card("Cadeira de Escritório Presidente X-Pro NR17 XT Office")];
        assert_eq!(narrower_term("cadeira XT Office X-Pro", "cadeira", &found), None, "o modelo já apareceu");
        assert_eq!(narrower_term("cadeira DT3 Forst", "cadeira", &[]).as_deref(), Some("DT3 Forst"), "sem cards: tira só o tipo");
        assert_eq!(narrower_term("Forst", "cadeira", &[]), None, "nada mais curto");
    }

    #[test]
    fn css_dentro_do_card_nao_vira_titulo() {
        let html = r#"<body><div class="card"><style>#trustvox-new { display: none !important; } .product-stars-container { display: none }</style>
            <a href="/produto/123456/cadeira-royce"><span>Cadeira Gamer DT3 Royce</span></a><span>R$ 2.474,90</span></div></body>"#;
        let cards = cards_from_html(html, "https://loja.com.br/busca?q=royce", 10, None);
        assert_eq!(cards[0].title, "Cadeira Gamer DT3 Royce");
    }

    #[test]
    fn le_json_embutido_e_aprende_o_link() {
        let html = r#"<script id="__NEXT_DATA__" type="application/json">{"props":{"data":[
            {"code":"111111","friendlyName":"monitor-lg-27","name":"Monitor LG 27 IPS 144Hz","price":"1299.99","priceWithDiscount":"1099.99","oldPrice":"0","available":"True"},
            {"code":"222222","friendlyName":"monitor-aoc-27","name":"Monitor AOC 27 Full HD 180Hz","price":"999.99","priceWithDiscount":"899.99","available":"True"},
            {"code":"333333","friendlyName":"monitor-off","name":"Monitor Esgotado 27 polegadas","price":"500","available":"False"}]}}</script>"#;
        assert!(cards_from_html(html, "https://loja.com.br/busca/monitor", 10, None).is_empty(), "sem link e sem modelo: nada");
        let reference: Vec<Card> = [("Monitor LG 27 IPS 144Hz", "111111/monitor-lg-27"), ("Monitor AOC 27 Full HD 180Hz", "222222/monitor-aoc-27")]
            .iter()
            .map(|(t, p)| Card { title: t.to_string(), url: format!("https://loja.com.br/produto/{p}"), ..Default::default() })
            .collect();
        let template = learn_json_url(html, &reference).unwrap();
        assert_eq!(template, "https://loja.com.br/produto/{code}/{friendlyName}");
        let cards = cards_from_html(html, "https://loja.com.br/busca/monitor", 10, Some(&template));
        assert_eq!(cards.len(), 2, "o esgotado fica de fora");
        assert_eq!(cards[0].url, "https://loja.com.br/produto/111111/monitor-lg-27");
        assert_eq!(sorted_prices(&cards[0]), vec![1099.99, 1299.99]);
    }

    /// Diagnóstico com uma página salva: SNIPR_HTML=arquivo.html SNIPR_URL=https://… cargo test -p snipr-core le_pagina_salva -- --ignored --nocapture
    #[test]
    #[ignore]
    fn le_pagina_salva() {
        let _ = tracing_subscriber::fmt().with_env_filter("snipr_core=debug").try_init();
        let html = std::fs::read_to_string(std::env::var("SNIPR_HTML").unwrap()).unwrap();
        let url = std::env::var("SNIPR_URL").unwrap();
        let doc = Html::parse_document(&html);
        let products = embedded_products(&doc);
        println!("produtos no JSON: {}", products.len());
        if let Some(p) = products.first() {
            println!("primeiro: {:?}", p.iter().filter(|(_, v)| !v.is_object() && !v.is_array()).take(8).collect::<Vec<_>>());
        }
        let base = Url::parse(&url).unwrap();
        let ctx = Ctx { here: format!("{}{}", base.origin().ascii_serialization(), base.path()), base: &base };
        let dom = dom_cards(&doc, &ctx, 80);
        let plain = cards_from_html(&html, &url, 80, None);
        println!("cards do HTML: {} | sem modelo: {} | modelo aprendido: {:?}", dom.len(), plain.len(), learn_json_url(&html, &plain));
        let names: Vec<String> = products.iter().take(3).filter_map(|p| p.get("name").and_then(Value::as_str).map(title_key)).collect();
        println!("chaves JSON: {names:?}\nchaves ref:  {:?}", plain.iter().take(3).map(|c| (title_key(&c.title), c.url.clone())).collect::<Vec<_>>());
        let cards = cards_from_html(&html, &url, 80, std::env::var("SNIPR_JSON_URL").ok().as_deref());
        println!("cards: {}", cards.len());
        for c in cards.iter().take(5) {
            println!("  {:?} | {} | {}", c.prices, c.title, c.url);
        }
    }

    #[test]
    fn le_json_ld_e_vtex() {
        let html = r#"<script type="application/ld+json">{"@type":"ItemList","itemListElement":[{"item":{"@type":"Product","name":"Cadeira X","url":"/p/cadeira-x","offers":{"price":"1299.90"}}}]}</script>"#;
        let cards = cards_from_html(html, "https://loja.com.br/s?q=cadeira", 10, None);
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].prices, vec!["R$ 1.299,90"]);
        assert_eq!(cards[0].url, "https://loja.com.br/p/cadeira-x");

        let vtex = r#"[{"productName":"Monitor 27","link":"https://loja.com.br/monitor-27/p","brand":"LG","items":[{"sellers":[
            {"commertialOffer":{"Price":900.0,"ListPrice":1000.0,"AvailableQuantity":3}},
            {"commertialOffer":{"Price":800.0,"ListPrice":800.0,"AvailableQuantity":0}}]}]}]"#;
        let cards = cards_from_vtex(vtex, 10);
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].prices, vec!["R$ 900,00"], "vendedor sem estoque fica de fora");
        assert_eq!(cards[0].old, vec!["R$ 1.000,00"]);
    }

    #[test]
    fn precos_e_textos() {
        assert_eq!(parse_brl("R$ 1.299,90"), Some(1299.9));
        assert_eq!(parse_brl("R$ 1.250"), Some(1250.0));
        assert_eq!(format_brl(1299.9), "R$ 1.299,90");
        assert_eq!(format_brl(5.0), "R$ 5,00");
        assert_eq!(tokens("Placa-mãe Gigabyte AM5 com Wi-Fi"), vec!["placa", "mae", "gigabyte", "am5"]);
        let card = Card { prices: vec!["R$ 941,16".into(), "R$ 799,99".into()], ..Default::default() };
        assert_eq!(sorted_prices(&card), vec![799.99, 941.16]);
    }

    #[test]
    fn grupos_do_plano_tiram_o_tipo_de_produto() {
        let alts = ["ThunderX3 Core XXL", "DT3 Royce", "XT Office X-Pro", "DT3 modelo Forst"].map(String::from);
        let g = plan_groups("Cadeiras", &alts, &[]);
        assert_eq!(g, vec![vec!["thunderx3", "core", "xxl"], vec!["dt3", "royce"], vec!["office", "xpro"], vec!["dt3", "forst"]]);
        let terms = ["monitor 27 polegadas 144hz".to_string()];
        assert_eq!(plan_groups("monitor", &[], &terms), vec![vec!["27", "polegadas", "144hz"]]);
        assert!(plan_groups("air fryer", &[], &["air fryer".to_string()]).is_empty());
    }

    #[test]
    fn lista_curta_da_vez_a_cada_alternativa() {
        let card = |t: &str| Card { title: t.into(), ..Default::default() };
        // A primeira busca enche a página; os modelos das outras vêm depois.
        let mut cards: Vec<Card> = (0..45).map(|i| card(&format!("Cadeira Gamer ThunderX3 Core Smart {i}"))).collect();
        cards.push(card("Cadeira de Rodas Dobrável"));
        cards.extend((0..10).map(|i| card(&format!("Cadeira Office DT3 Vita {i}"))));
        cards.push(card("Cadeira Gamer DT3 Sports Royce Ashen"));
        cards.push(card("Cadeira Office Xt Racer - X-pro Xto-003s"));
        cards.push(card("Cadeira Gamer Thunderx3 Core Xxl Modern Azul"));
        let refs: Vec<&Card> = cards.iter().collect();
        let alts = ["ThunderX3 Core XXL", "DT3 Royce", "XT Office X-Pro", "DT3 Forst"].map(String::from);
        let short = shortlist(&refs, &plan_groups("cadeira", &alts, &[]), "cadeira", true, &[], 12);
        let titles: Vec<&str> = short.iter().map(|c| c.title.as_str()).collect();
        assert!(short.len() <= 12);
        assert_eq!(titles[0], "Cadeira Gamer Thunderx3 Core Xxl Modern Azul", "{titles:#?}");
        assert!(titles.contains(&"Cadeira Gamer DT3 Sports Royce Ashen"), "{titles:#?}");
        assert!(titles.contains(&"Cadeira Office Xt Racer - X-pro Xto-003s"), "{titles:#?}");
        assert!(!titles.contains(&"Cadeira de Rodas Dobrável"));
        assert!(!titles.iter().any(|t| t.contains("Vita")), "só a marca bate: é outro modelo: {titles:#?}");

        // Sem alternativas, especificações que o título não cita não derrubam o card.
        let monitors = [card("Monitor Gamer LG 27 IPS 144Hz"), card("Monitor AOC 24 75Hz"), card("Suporte de Mesa")];
        let refs: Vec<&Card> = monitors.iter().collect();
        let terms = ["monitor 27 polegadas 144hz".to_string()];
        let short = shortlist(&refs, &plan_groups("monitor", &[], &terms), "monitor", false, &[], 10);
        assert_eq!(short[0].title, "Monitor Gamer LG 27 IPS 144Hz");
        assert!(!short.iter().any(|c| c.title.starts_with("Suporte")));

        // A busca curta "Royce" traz outros produtos: sem "cadeira", ficam de fora.
        let mixed = [card("Cadeira Gamer DT3 Royce Ashen"), card("Cadeira Gamer DT3 Nero"), card("Carteira de couro Royce"), card("Royce Leather Padfolio")];
        let refs: Vec<&Card> = mixed.iter().collect();
        let short = shortlist(&refs, &plan_groups("cadeira", &alts, &[]), "cadeira", true, &[], 10);
        assert_eq!(short.iter().map(|c| c.title.as_str()).collect::<Vec<_>>(), vec!["Cadeira Gamer DT3 Royce Ashen"]);
        // Títulos que não usam a palavra do tipo de produto: a regra não vale.
        let phones = [card("Smartphone Samsung Galaxy S24 128GB"), card("Capa para Galaxy S23")];
        let refs: Vec<&Card> = phones.iter().collect();
        let alts = ["Samsung Galaxy S24".to_string()];
        assert_eq!(shortlist(&refs, &plan_groups("celular", &alts, &[]), "celular", true, &[], 10).len(), 1);
        // Mesmo quando a loja usa a palavra, o sinônimo com o modelo inteiro passa.
        let phones = [card("Celular Samsung Galaxy A15"), card("Smartphone Samsung Galaxy S24 128GB"), card("Película S24")];
        let refs: Vec<&Card> = phones.iter().collect();
        let short = shortlist(&refs, &plan_groups("celular", &alts, &[]), "celular", true, &[], 10);
        let titles: Vec<&str> = short.iter().map(|c| c.title.as_str()).collect();
        assert_eq!(titles[0], "Smartphone Samsung Galaxy S24 128GB");
        assert!(!titles.contains(&"Película S24"), "{titles:?}");
    }

    #[test]
    fn cobertura_compara_por_link_ou_titulo() {
        let a = Card { title: "Monitor LG 27 IPS 144Hz".into(), url: "https://l.com/produto/1/x".into(), ..Default::default() };
        let b = Card { title: "Outro produto qualquer".into(), url: "https://l.com/produto/2/y".into(), ..Default::default() };
        let got = vec![Card { title: "Monitor LG 27 IPS 144Hz".into(), url: "https://l.com/outro".into(), ..Default::default() }];
        assert_eq!(coverage(&[a, b], &got), 0.5);
    }
}
