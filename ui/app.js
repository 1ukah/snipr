// Snipr — interface. Sem framework, sem build.
// Regra: dados vindos da web (títulos, vendedores...) só entram no DOM via
// textContent, nunca innerHTML.
'use strict';

const T = window.__TAURI__;
const invoke = (cmd, args) => T.core.invoke(cmd, args);
const $ = (s, el = document) => el.querySelector(s);
const $$ = (s, el = document) => [...el.querySelectorAll(s)];
const BRL = new Intl.NumberFormat('pt-BR', { style: 'currency', currency: 'BRL' });

function el(tag, props = {}, ...kids) {
  const e = document.createElement(tag);
  for (const [k, v] of Object.entries(props)) {
    if (k === 'class') e.className = v;
    else if (k === 'text') e.textContent = v;
    else if (k.startsWith('on')) e.addEventListener(k.slice(2), v);
    else if (v !== undefined && v !== null && v !== false) e.setAttribute(k, v === true ? '' : v);
  }
  for (const k of kids.flat()) if (k != null) e.append(k instanceof Node ? k : document.createTextNode(String(k)));
  return e;
}

const state = {
  settings: null,
  sites: [],
  selected: new Set(),
  job: null,
  listings: [],
  agents: new Map(),
  filter: 'all',
  condition: 'any',
  mode: 'single',
  started: 0,
};

let toastTimer;
function toast(msg) {
  const t = $('#toast');
  t.textContent = msg;
  t.hidden = false;
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => (t.hidden = true), 4500);
}
const fail = (e) => toast(String(e));

// ---------------- navegação ----------------
$$('.nav').forEach((b) =>
  b.addEventListener('click', () => {
    $$('.nav').forEach((x) => x.classList.toggle('on', x === b));
    $$('.view').forEach((v) => v.classList.toggle('on', v.id === 'view-' + b.dataset.view));
    if (b.dataset.view === 'settings') refreshSecrets();
  })
);

// ---------------- busca ----------------
$$('.seg').forEach((seg) =>
  seg.addEventListener('click', (e) => {
    const b = e.target.closest('button');
    if (!b) return;
    $$('button', seg).forEach((x) => x.classList.toggle('on', x === b));
    state[seg.dataset.name] = b.dataset.v;
    renderList();
  })
);

function renderChips() {
  const box = $('#site-chips');
  box.replaceChildren(
    ...state.sites.map((s) =>
      el('button', {
        type: 'button',
        class: 'chip' + (state.selected.has(s.id) ? ' on' : ''),
        title: `${s.domain} · ${s.sells}${s.requires_login ? ' · exige login' : ''}`,
        onclick: () => {
          state.selected.has(s.id) ? state.selected.delete(s.id) : state.selected.add(s.id);
          renderChips();
          persistSites();
        },
      }, s.name, s.requires_login ? el('span', { class: 'lock', text: '●' }) : null)
    )
  );
}

let persistTimer;
function persistSites() {
  clearTimeout(persistTimer);
  persistTimer = setTimeout(() => {
    state.settings.enabled_sites = [...state.selected];
    invoke('save_settings', { settings: state.settings }).catch(fail);
  }, 600);
}

$('#search-form').addEventListener('submit', async (e) => {
  e.preventDefault();
  if (state.job) {
    invoke('cancel_search', { job: state.job });
    $('#job-status').textContent = 'cancelando…';
    return;
  }
  const query = $('#q').value.trim();
  if (!query) return;
  const max = parseFloat($('#max-price').value);
  const req = {
    query,
    condition: state.condition,
    mode: state.mode,
    sites: [...state.selected],
    max_price: Number.isFinite(max) && max > 0 ? max : null,
    notes: $('#notes').value.trim() || null,
  };
  try {
    resetBoard();
    state.job = await invoke('start_search', { req });
    state.started = Date.now();
    setRunning(true);
  } catch (err) {
    fail(err);
  }
});

function setRunning(on) {
  const b = $('#go');
  b.textContent = on ? 'Parar' : 'Caçar';
  b.classList.toggle('stop', on);
  $('#job-status').textContent = on ? 'caçando…' : $('#job-status').textContent;
}

function resetBoard() {
  state.listings = [];
  state.agents.clear();
  $('#log').replaceChildren();
  $('#human').replaceChildren();
  renderAgents();
  renderList();
}

// ---------------- eventos do motor ----------------
T.event.listen('snipr-event', ({ payload: ev }) => {
  if (ev.job !== state.job) return;
  switch (ev.type) {
    case 'started':
      for (const a of ev.agents) state.agents.set(a.id, { ...a, status: 'wait', last: 'na fila' });
      renderAgents();
      break;
    case 'log': {
      const a = state.agents.get(ev.agent);
      if (a) { a.status = 'run'; a.last = ev.message; }
      addLog(a ? a.label : '?', ev.message);
      renderAgents();
      break;
    }
    case 'listing':
      state.listings.push(ev.listing);
      renderList();
      break;
    case 'needs_human':
      showHuman(ev);
      break;
    case 'human_resolved':
      $(`#human [data-agent="${CSS.escape(ev.agent)}"]`)?.remove();
      break;
    case 'agent_finished': {
      const a = state.agents.get(ev.agent);
      if (a) { a.status = ev.ok ? 'ok' : 'fail'; a.last = ev.summary; }
      addLog(a ? a.label : '?', (ev.ok ? '✓ ' : '✗ ') + ev.summary);
      renderAgents();
      break;
    }
    case 'finished': {
      const secs = (ev.elapsed_ms / 1000).toFixed(0);
      $('#job-status').textContent = `${ev.total} anúncio(s) · ${secs}s${ev.cancelled ? ' · cancelado' : ''}`;
      state.job = null;
      setRunning(false);
      $('#human').replaceChildren();
      break;
    }
  }
});

function addLog(who, msg) {
  const log = $('#log');
  const time = new Date().toLocaleTimeString('pt-BR', { hour12: false });
  log.append(el('li', {}, el('span', { class: 't', text: time + ' ' }), el('span', { class: 'who', text: who + ' ' }), msg));
  while (log.children.length > 300) log.firstChild.remove();
  log.scrollTop = log.scrollHeight;
}

const STATE_LABEL = { wait: 'fila', run: 'ativo', ok: 'pronto', fail: 'falhou' };
function renderAgents() {
  $('#agents').replaceChildren(
    ...[...state.agents.values()].map((a) =>
      el('div', { class: 'agent' + (a.status === 'run' ? ' running' : '') },
        el('div', { class: 'top' },
          el('span', { class: 'name', text: a.label }),
          el('span', { class: 'state ' + a.status, text: STATE_LABEL[a.status] })),
        el('div', { class: 'last', text: a.last || '' }))
    )
  );
}

function showHuman(ev) {
  const a = state.agents.get(ev.agent);
  const card = el('div', { class: 'human', 'data-agent': ev.agent },
    el('div', { class: 'eyebrow', text: 'Precisa de você · ' + (a ? a.label : '') }),
    el('p', { text: ev.reason }),
    el('div', { class: 'acts' },
      el('button', { text: 'Resolvido', onclick: () => invoke('resolve_human', { agent: ev.agent, ok: true }) }),
      el('button', { text: 'Pular site', onclick: () => invoke('resolve_human', { agent: ev.agent, ok: false }) })));
  $('#human').append(card);
  toast('Um agente precisa da sua ajuda: a janela do navegador foi aberta.');
}

// ---------------- resultados ----------------
$$('.tab').forEach((t) =>
  t.addEventListener('click', () => {
    state.filter = t.dataset.f;
    $$('.tab').forEach((x) => x.classList.toggle('on', x === t));
    renderList();
  })
);

const COND = { new: 'NOVO', used: 'USADO', unknown: '?' };
function renderList() {
  const all = [...state.listings].sort((a, b) => a.price - b.price);
  const news = all.filter((l) => l.condition === 'new');
  const used = all.filter((l) => l.condition === 'used');
  const counts = { all: all.length, new: news.length, used: used.length };
  $$('.tab').forEach((t) => ($('.n', t).textContent = counts[t.dataset.f]));

  stub('#best-new', news[0]);
  stub('#best-used', used[0]);

  const shown = state.filter === 'new' ? news : state.filter === 'used' ? used : all;
  $('#empty').hidden = shown.length > 0;
  $('#list').replaceChildren(
    ...shown.map((l, i) =>
      el('li', { class: 'item' + (i === 0 ? ' first' : '') },
        el('div', { class: 'line' },
          el('span', { class: 'title', title: l.title, text: l.title }),
          el('span', { class: 'dots' }),
          el('span', { class: 'price', text: BRL.format(l.price) })),
        el('div', { class: 'meta' },
          el('span', { class: 'stamp', text: l.site_name }),
          el('span', { class: 'tag ' + l.condition, text: COND[l.condition] || '?' }),
          [l.location, l.shipping, l.seller].filter(Boolean).join(' · ') || null,
          l.notes ? el('span', { class: 'muted', text: l.notes }) : null,
          el('button', { class: 'open', text: 'abrir ↗', onclick: () => openUrl(l.url) })))
    )
  );
}

function stub(sel, l) {
  const box = $(sel);
  box.classList.toggle('hit', !!l);
  $('.stub-price', box).textContent = l ? BRL.format(l.price) : '—';
  $('.stub-meta', box).textContent = l ? `${l.site_name} · ${l.title}` : '';
  box.onclick = l ? () => openUrl(l.url) : null;
}

function openUrl(url) {
  if (!/^https?:\/\//.test(url)) return;
  T.opener.openUrl(url).catch(fail);
}

// ---------------- contas ----------------
function renderAccounts() {
  const sites = [...state.sites].sort((a, b) => Number(b.requires_login) - Number(a.requires_login));
  $('#site-logins').replaceChildren(
    ...sites.map((s) =>
      el('div', { class: 'row' },
        el('div', { class: 'grow' },
          el('div', { class: 'nm', text: s.name }),
          el('div', { class: 'small muted', text: s.domain + (s.requires_login ? ' · exige login para buscar' : ' · login opcional') })),
        el('button', { class: 'btn', text: 'Entrar', onclick: () => openLogin(s) })))
  );
  const creds = state.settings.credential_domains || [];
  $('#cred-list').replaceChildren(
    ...(creds.length
      ? creds.map((d) =>
          el('div', { class: 'row' },
            el('div', { class: 'grow' },
              el('div', { class: 'nm mono', text: d }),
              el('div', { class: 'small muted', text: 'usuário e senha no cofre do sistema' })),
            el('button', { class: 'link', text: 'remover', onclick: () => removeCred(d) })))
      : [el('div', { class: 'row small muted', text: 'Nenhuma credencial guardada.' })])
  );
}

async function openLogin(site) {
  try {
    await invoke('open_login', { site: site.id });
    $('#login-text').textContent = `Faça login em ${site.name} normalmente (inclusive verificação em 2 etapas). Depois clique em "Concluí": a sessão fica salva.`;
    $('#login-banner').hidden = false;
  } catch (e) {
    fail(e);
  }
}

$('#login-done').addEventListener('click', async () => {
  await invoke('close_login').catch(fail);
  $('#login-banner').hidden = true;
  toast('Sessão salva no perfil do Snipr.');
});

$('#cred-form').addEventListener('submit', async (e) => {
  e.preventDefault();
  const f = e.target;
  const data = { domain: f.domain.value, username: f.username.value, password: f.password.value };
  f.password.value = '';
  try {
    const d = await invoke('add_credential', data);
    f.reset();
    state.settings = await invoke('get_settings');
    renderAccounts();
    toast(`Credencial de ${d} guardada no cofre.`);
  } catch (err) {
    fail(err);
  }
});

async function removeCred(d) {
  await invoke('remove_credential', { domain: d }).catch(fail);
  state.settings = await invoke('get_settings');
  renderAccounts();
}

// ---------------- ajustes ----------------
const form = $('#settings-form');
const MODEL_HINT = {
  claude_code: 'padrão do Claude Code (ex.: sonnet, opus)',
  anthropic: 'claude-opus-5-5',
  openai_compat: 'nome do modelo no provedor',
};
const ENGINE_LABEL = { claude_code: 'Claude Code · assinatura', anthropic: 'Anthropic API', openai_compat: 'API compatível OpenAI' };

function fillSettings(s) {
  for (const [k, v] of Object.entries(s)) {
    const f = form.elements[k];
    if (!f || f instanceof RadioNodeList) continue;
    if (f.type === 'checkbox') f.checked = !!v;
    else f.value = v ?? '';
  }
  $('#base-preset').value = [...$('#base-preset').options].some((o) => o.value === s.openai_base_url) ? s.openai_base_url : '';
  syncProvider();
}

function syncProvider() {
  const p = form.elements.provider.value;
  $$('.for-claude_code, .for-anthropic, .for-openai_compat, .for-openai', form).forEach((n) => {
    const show = n.classList.contains('for-' + p) || (p === 'openai_compat' && n.classList.contains('for-openai'));
    n.hidden = !show;
  });
  $('#model-input').placeholder = MODEL_HINT[p];
}
form.elements.provider.addEventListener('change', syncProvider);
$('#base-preset').addEventListener('change', (e) => {
  if (e.target.value) form.elements.openai_base_url.value = e.target.value;
});

form.addEventListener('submit', async (e) => {
  e.preventDefault();
  const s = { ...state.settings };
  for (const f of form.elements) {
    if (!f.name || !(f.name in s)) continue;
    if (f.type === 'checkbox') s[f.name] = f.checked;
    else if (f.type === 'number') s[f.name] = parseInt(f.value, 10) || 0;
    else s[f.name] = f.value.trim();
  }
  try {
    await invoke('save_settings', { settings: s });
    state.settings = await invoke('get_settings');
    fillSettings(state.settings);
    $('#engine-label').textContent = ENGINE_LABEL[state.settings.provider];
    $('#save-msg').textContent = 'salvo ' + new Date().toLocaleTimeString('pt-BR');
  } catch (err) {
    fail(err);
  }
});

// Campos de segredo: o valor colado vai direto ao cofre e o campo é limpo.
// Não existe caminho para ler o valor de volta.
async function storeSecret(box, value) {
  const input = $('input', box);
  input.value = '';
  value = (value || '').trim();
  if (!value) return;
  try {
    await invoke('set_secret', { name: box.dataset.name, value });
    toast('Guardado no cofre do sistema.');
  } catch (e) {
    fail(e);
  }
  refreshSecrets();
}

$$('.secret').forEach((box) => {
  const input = $('input', box);
  input.addEventListener('paste', (e) => {
    e.preventDefault();
    storeSecret(box, e.clipboardData.getData('text'));
  });
  input.addEventListener('keydown', (e) => {
    if (e.key === 'Enter') {
      e.preventDefault();
      storeSecret(box, input.value);
    }
  });
  input.addEventListener('blur', () => input.value && storeSecret(box, input.value));
  $('[data-act=del]', box).addEventListener('click', async () => {
    await invoke('delete_secret', { name: box.dataset.name }).catch(fail);
    refreshSecrets();
  });
});

async function refreshSecrets() {
  const boxes = $$('.secret');
  const status = await invoke('secret_status', { names: boxes.map((b) => b.dataset.name) }).catch(() => ({}));
  for (const b of boxes) {
    const has = !!status[b.dataset.name];
    const st = $('.secret-state', b);
    st.textContent = has ? '● guardado' : '○ vazio';
    st.classList.toggle('set', has);
    $('input', b).placeholder = has ? '•••••••••••• cole outro para trocar' : 'cole aqui (fica oculto)';
    $('[data-act=del]', b).hidden = !has;
  }
}

// ---------------- início ----------------
(async function boot() {
  try {
    state.settings = await invoke('get_settings');
    state.sites = await invoke('list_sites');
    state.selected = new Set(state.settings.enabled_sites);
    renderChips();
    renderList();
    renderAccounts();
    fillSettings(state.settings);
    refreshSecrets();
    $('#engine-label').textContent = ENGINE_LABEL[state.settings.provider];
    $('#q').focus();
  } catch (e) {
    fail(e);
  }
})();
