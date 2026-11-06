//! Scripts executados num "isolated world" — um contexto JS separado que
//! enxerga o DOM mas é invisível para os scripts da própria página.

/// Lista elementos interativos visíveis e guarda referências numeradas
/// (privadas ao isolated world) para `click`/`type`.
pub const SNAPSHOT: &str = r#"
(maxItems, filter) => {
  const W = window;
  W.__snipr = W.__snipr || {};
  const refs = [];
  W.__snipr.refs = refs;
  const vis = (el) => {
    const r = el.getBoundingClientRect();
    if (r.width < 2 || r.height < 2) return false;
    const s = getComputedStyle(el);
    return s.visibility !== 'hidden' && s.display !== 'none' && parseFloat(s.opacity || '1') > 0.05;
  };
  const clean = (s) => (s || '').replace(/\s+/g, ' ').trim();
  const label = (el) => {
    let t = clean(el.innerText);
    if (!t) t = clean(el.getAttribute('aria-label'));
    if (!t) t = clean(el.getAttribute('placeholder'));
    if (!t) t = clean(el.getAttribute('title'));
    if (!t) { const img = el.querySelector && el.querySelector('img[alt]'); if (img) t = clean(img.alt); }
    if (!t && el.name) t = clean(el.name);
    return t.slice(0, 110);
  };
  const kind = (el) => {
    const tag = el.tagName.toLowerCase();
    if (tag === 'a') return 'link';
    if (tag === 'input') return 'input:' + (el.type || 'text');
    if (tag === 'select') return 'select';
    if (tag === 'textarea') return 'textarea';
    if (tag === 'button') return 'botão';
    return el.getAttribute('role') || tag;
  };
  const sel = 'a[href],button,input:not([type=hidden]),select,textarea,[role=button],[role=link],[role=tab],[role=checkbox],[role=radio],[role=option],[role=menuitem],[role=combobox],[contenteditable=true],summary';
  const f = filter ? String(filter).toLowerCase() : null;
  const lines = [];
  let total = 0;
  for (const el of document.querySelectorAll(sel)) {
    if (!vis(el)) continue;
    const t = label(el);
    const k = kind(el);
    if (!t && k === 'link') continue;
    if (f && !t.toLowerCase().includes(f)) continue;
    total++;
    if (lines.length >= maxItems) continue;
    const id = refs.push(el) - 1;
    let line = `[${id}] ${k} "${t}"`;
    if (el.tagName === 'A') {
      try { const u = new URL(el.href); line += ` → ${u.pathname.slice(0, 60)}`; } catch (e) {}
    }
    if (el.tagName === 'INPUT' || el.tagName === 'TEXTAREA') {
      const v = el.type === 'password' ? (el.value ? '••••' : '') : clean(el.value).slice(0, 40);
      if (v) line += ` valor="${v}"`;
    }
    if (el.tagName === 'SELECT') {
      const o = el.options[el.selectedIndex];
      if (o) line += ` selecionado="${clean(o.text).slice(0, 40)}"`;
    }
    lines.push(line);
  }
  let head = `URL: ${location.href}\nTítulo: ${document.title}\n`;
  const more = total > lines.length ? `\n… ${total - lines.length} elementos omitidos (use filter para achar algo específico)` : '';
  return head + (lines.length ? lines.join('\n') : '(nenhum elemento interativo visível)') + more;
}
"#;

/// Heurística genérica de "cards" de anúncio: parte de cada preço em reais e
/// sobe até o menor ancestral que contém um link de produto. Funciona na
/// maioria dos marketplaces sem seletores específicos por site.
pub const EXTRACT_LISTINGS: &str = r#"
(maxCards) => {
  const PRICE = /R\$\s*\d{1,3}(?:\.\d{3})*(?:,\d{2})?|R\$\s*\d+(?:,\d{2})?/g;
  const HAS_PRICE = /R\$\s*\d/;
  const TRACK = /click|clicks|clk|publicidade|adclick|redirect|\/count\b|doubleclick|taboola|outbrain|criteo|rtb-|\/ads?\//i;
  const clean = (s) => (s || '').replace(/\s+/g, ' ').trim();
  const norm = (s) => clean(s).replace(/R\$\s*/g, 'R$ ').replace(/(\d)\s*,\s*(\d{2})/g, '$1,$2');
  const here = location.origin + location.pathname;
  // Link de produto: http(s), não é rastreamento, não é a própria página de busca.
  const productUrl = (a) => {
    let u; try { u = new URL(a.href, location.href); } catch (e) { return null; }
    if (!/^https?:$/.test(u.protocol) || TRACK.test(u.hostname + u.pathname)) return null;
    if (u.origin + u.pathname === here || u.pathname.length < 2) return null;
    return u;
  };
  // 1) Menores elementos cujo texto contém um preço.
  const priceEls = [];
  for (const e of document.body.querySelectorAll('*')) {
    if (e.children.length > 6) continue;
    const t = e.innerText;
    if (!t || t.length > 60 || !HAS_PRICE.test(t)) continue;
    if ([...e.children].some((c) => HAS_PRICE.test(c.innerText || ''))) continue;
    priceEls.push(e);
  }
  const linkScore = (a, u) => u.pathname.length
    + (/\d{5,}|\/dp\/|\/p\/|produto|item|MLB/i.test(u.pathname) ? 60 : 0)
    + (a.querySelector('img, h1, h2, h3, h4') ? 30 : 0)
    - (u.pathname.length < 12 ? 80 : 0)
    - (/^\/@|perfil|profile|seller|vendedor|\/loja\/|\/store\/|\/user|\/b$|\/s$|\/deals|\/busca|\/search/i.test(u.pathname) ? 150 : 0);
  // 2) De cada preço, sobe até o menor ancestral que contém um link de produto.
  const cards = [], boxes = new Set(), urls = new Set();
  for (const p of priceEls) {
    if (cards.length >= maxCards) break;
    let box = p, link = null, extraUp = 0;
    for (let i = 0; i < 10 && box && box !== document.body; i++) {
      const anchors = box.tagName === 'A' ? [box] : [...box.querySelectorAll('a[href]')];
      const prods = anchors.map((a) => [a, productUrl(a)]).filter(([, u]) => u);
      if (prods.length) {
        // Vários produtos distintos = subimos demais (é uma lista, não um card).
        const distinct = new Set(prods.map(([, u]) => u.pathname));
        if (distinct.size > 3) { if (!link) box = null; else box = box._prev; break; }
        prods.sort((x, y) => linkScore(y[0], y[1]) - linkScore(x[0], x[1]));
        link = prods[0];
        // Só achou link fraco (vendedor, categoria)? Sobe um pouco atrás do produto.
        if (linkScore(link[0], link[1]) < 40 && extraUp < 2 && (box.innerText || '').length < 800) {
          extraUp++;
          const parent = box.parentElement;
          if (parent) parent._prev = box;
          box = parent;
          continue;
        }
        break;
      }
      box = box.parentElement;
    }
    if (!box || !link || boxes.has(box)) continue;
    if ((box.innerText || '').length > 1500) continue;
    const r = box.getBoundingClientRect();
    if (r.width < 40 || r.height < 20) continue;
    const u = link[1];
    let url = u.origin + u.pathname;
    if (u.pathname.length < 25 && u.search) url += u.search.slice(0, 120);
    if (urls.has(url)) continue;
    boxes.add(box); urls.add(url);
    const text = norm(box.innerText);
    const prices = [...new Set((text.match(PRICE) || []).map((x) => x.replace(/\s+/g, ' ')))].slice(0, 4);
    // Título: headings e atributos title/alt valem mais; depois o texto mais longo sem preço.
    let title = '', best = -1;
    for (const x of [link[0], ...box.querySelectorAll('*')]) {
      let t = clean(x.getAttribute('title') || (x.tagName === 'IMG' ? x.getAttribute('alt') : '')), bonus = 30;
      if (!t && /^H[1-6]$/.test(x.tagName)) { t = clean(x.innerText); bonus = 60; }
      if (!t && x.children.length <= 2) { t = clean(x.innerText); bonus = 0; }
      if (!t || t.length < 8 || t.length > 200 || HAS_PRICE.test(t)) continue;
      if (/^(anúncio patrocinado|patrocinado|oferta|ver mais|adicionar)/i.test(t)) t = t.replace(/^anúncio patrocinado\s*[–-]\s*/i, '');
      const score = t.length + bonus;
      if (score > best) { best = score; title = t; }
    }
    if (!title) continue;
    const old = [...box.querySelectorAll('s, del, [class*="old"], [class*="previous"], [class*="original"]')]
      .map((e) => clean(e.innerText)).filter((t) => HAS_PRICE.test(t)).slice(0, 2);
    const extra = text.replace(title, '').replace(PRICE, ' ').replace(/\s+/g, ' ').trim().slice(0, 170);
    cards.push({ title: title.slice(0, 160), prices, old, url, extra });
  }
  // 3) Fallback: há preços, mas nenhum card expõe link próprio (ex.: Shopee usa
  //    cliques via JS). Devolve os cards mesmo assim, marcados como "sem link".
  if (!cards.length) {
    const seenBox = new Set();
    for (const p of priceEls) {
      if (cards.length >= maxCards) break;
      let box = p;
      for (let i = 0; i < 6 && box.parentElement && box.parentElement !== document.body; i++) {
        const t = box.innerText || '';
        if (t.length > 40 && t.split('\n').some((l) => l.trim().length >= 15 && !HAS_PRICE.test(l))) break;
        box = box.parentElement;
      }
      const t = box.innerText || '';
      if (seenBox.has(box) || t.length > 500 || t.length < 20) continue;
      seenBox.add(box);
      const text = norm(t);
      const title = t.split('\n').map(clean).filter((l) => l.length >= 15 && !HAS_PRICE.test(l)).sort((a, b) => b.length - a.length)[0];
      if (!title) continue;
      const prices = [...new Set((text.match(PRICE) || []).map((x) => x.replace(/\s+/g, ' ')))].slice(0, 4);
      cards.push({ title: title.slice(0, 160), prices, old: [], url: 'SEM LINK (clique no card para obter)', extra: text.replace(title, '').replace(PRICE, ' ').replace(/\s+/g, ' ').trim().slice(0, 120) });
    }
  }
  if (!cards.length) return `Nenhum card com preço encontrado em ${location.href}. Tente snapshot, scroll, ou read_text.`;
  return `URL: ${location.href}\n${cards.length} cards:\n` + cards.map((c, i) =>
    `#${i + 1} | ${c.prices.join(' / ')}${c.old.length ? ' (riscado: ' + c.old.join(', ') + ')' : ''} | ${c.title} | ${c.url}${c.extra ? ' | ' + c.extra : ''}`
  ).join('\n');
}

"#;

pub const READ_TEXT: &str = r#"
(max) => {
  const m = document.querySelector('main, [role=main], #main, #content') || document.body;
  const t = (m.innerText || '').replace(/[ \t]+/g, ' ').replace(/\n{3,}/g, '\n\n').trim();
  return `URL: ${location.href}\nTítulo: ${document.title}\n\n` + t.slice(0, max) + (t.length > max ? '\n…(cortado)' : '');
}
"#;

/// Rola o elemento até o centro da tela e devolve suas coordenadas.
pub const LOCATE_REF: &str = r#"
(ref, scroll) => {
  const el = window.__snipr && window.__snipr.refs && window.__snipr.refs[ref];
  if (!el || !el.isConnected) return null;
  if (scroll) el.scrollIntoView({ block: 'center', inline: 'center', behavior: 'instant' });
  const r = el.getBoundingClientRect();
  const blank = el.tagName === 'A' && (el.target === '_blank');
  return { x: r.left + r.width / 2, y: r.top + r.height / 2, w: r.width, h: r.height,
           href: el.tagName === 'A' ? el.href : null, blank,
           tag: el.tagName, type: el.type || null };
}
"#;

pub const SELECT_ALL_IN_REF: &str = r#"
(ref) => {
  const el = window.__snipr && window.__snipr.refs && window.__snipr.refs[ref];
  if (!el) return false;
  el.focus();
  if (typeof el.select === 'function') { el.select(); return true; }
  return false;
}
"#;

/// Detecta páginas de desafio / bloqueio anti-bot.
pub const DETECT_CHALLENGE: &str = r#"
(() => {
  const t = (document.title || '').toLowerCase();
  if (/just a moment|um momento|attention required|checking your browser|verificando se você é humano|verify you are human/.test(t)) return 'cloudflare';
  if (document.querySelector('#challenge-form, #challenge-stage, #cf-challenge-running, #challenge-running, .cf-turnstile-wrapper')) return 'cloudflare';
  const big = (el) => { const r = el.getBoundingClientRect(); return r.width > 50 && r.height > 50; };
  for (const f of document.querySelectorAll('iframe')) {
    const s = f.src || '';
    if (/challenges\.cloudflare\.com/.test(s) && big(f) && document.body.innerText.length < 3000) return 'cloudflare';
    if (/recaptcha\/(api2|enterprise)\/bframe|hcaptcha\.com.*challenge/.test(s) && big(f)) return 'captcha';
  }
  if (document.querySelector('#px-captcha, [id^="px-captcha"]')) return 'perimeterx';
  if (/access denied|acesso negado|robot or human|are you a robot|você é um robô|não é possível acessar|forbidden|too many requests/.test(t)) return 'blocked';
  // Páginas de bloqueio na borda (Akamai etc.): curtas, com 403/429 ou "tente novamente em".
  const body = (document.body && document.body.innerText) || '';
  if (body.length < 1500 && /(erro|error)\s*(403|429)|tente novamente em \d+ minuto|try again in \d+ minute|request blocked|reference #?\d|ID: \d\.[0-9a-f]+\.\d+/i.test(body)) return 'blocked';
  return null;
})()
"#;
