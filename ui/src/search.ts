import { api } from './api';
import type { ConditionFilter } from './bindings/ConditionFilter';
import type { SearchEvent } from './bindings/SearchEvent';
import type { SearchPlan } from './bindings/SearchPlan';
import type { SearchRequest } from './bindings/SearchRequest';
import { $, $$, el, errText, fold, icon, toast } from './dom';
import { closePop, togglePop } from './popover';
import { type Cond, saveSettings, siteById, state } from './state';
import { t } from './strings';

const condFilter = (c: Iterable<Cond>): ConditionFilter => {
  const s = new Set(c);
  return s.has('new') && s.has('used') ? 'any' : s.has('used') ? 'used' : s.has('new') ? 'new' : 'any';
};
const condsOf = (f: ConditionFilter): Cond[] => (f === 'any' ? ['new', 'used'] : [f]);
const isRunning = () => state.running.size > 0;
const searched = () => state.searchedSites.size > 0 || state.listings.length > 0;

/* ---------- filters ---------- */

const inPrice = (p: number) => (state.lo == null || p >= state.lo) && (state.hi == null || p <= state.hi);
const inCond = (c: string) => state.cond.has(c as Cond) || c === 'unknown';
const inSite = (id: string) => state.selected.has(id);

let textWords: string[] = [];
const inIntl = (l: { international: boolean }) => state.showIntl || !l.international;
const inText = (l: { title: string; site_name: string }) => {
  if (!textWords.length) return true;
  const hay = fold(`${l.title} ${l.site_name}`);
  return textWords.every((w) => hay.includes(w));
};

function visible() {
  return state.listings.filter((l) => inIntl(l) && inText(l) && inPrice(l.price) && inCond(l.condition) && inSite(l.site_id)).sort((a, b) => a.price - b.price);
}

function check(on: boolean, onclick: () => void, kids: (Node | string | null)[]) {
  return el('button', { type: 'button', class: 'ck', role: 'checkbox', 'aria-checked': String(on), onclick }, el('span', { class: 'box' }, icon('i-check')), ...kids);
}

function persistFilters() {
  saveSettings({ enabled_sites: [...state.selected], conditions: [...state.cond] });
}

function renderCond() {
  const base = state.listings.filter((l) => inIntl(l) && inText(l) && inPrice(l.price) && inSite(l.site_id));
  $('#f-cond').replaceChildren(
    ...(['new', 'used'] as Cond[]).map((k) =>
      check(
        state.cond.has(k),
        () => {
          if (state.cond.has(k)) state.cond.delete(k);
          else state.cond.add(k);
          if (!state.cond.size) state.cond.add(k);
          persistFilters();
          render();
        },
        [el('span', { class: 'nm', text: k === 'new' ? t.new : t.used }), el('span', { class: 'n', text: state.searchedCond.has(k) ? base.filter((l) => l.condition === k).length : '' })],
      ),
    ),
  );
}

function renderOrigin() {
  const n = state.listings.filter((l) => l.international && inText(l) && inPrice(l.price) && inCond(l.condition) && inSite(l.site_id)).length;
  $('#f-origin').replaceChildren(
    check(
      state.showIntl,
      () => {
        state.showIntl = !state.showIntl;
        saveSettings({ show_international: state.showIntl });
        render();
      },
      [el('span', { class: 'nm', text: t.international, title: t.intlTip }), el('span', { class: 'n', text: state.listings.length ? n : '' })],
    ),
  );
}

let storeQ = '';
let storeList: HTMLElement | null = null;

function storesSummary() {
  const n = state.selected.size;
  const total = state.sites.length;
  if (n === total) return t.allStores;
  if (n === 0) return t.noStore;
  if (n === 1) return siteById([...state.selected][0])?.name ?? '';
  return t.storesCount(n, total);
}

function paintStoreList() {
  if (!storeList) return;
  const base = state.listings.filter((l) => inIntl(l) && inText(l) && inPrice(l.price) && inCond(l.condition));
  const q = fold(storeQ.trim());
  const shown = state.sites.filter((s) => fold(s.name).includes(q) || fold(s.domain).includes(q));
  storeList.replaceChildren(
    ...(shown.length
      ? shown.map((s) =>
          check(
            state.selected.has(s.id),
            () => {
              if (state.selected.has(s.id)) state.selected.delete(s.id);
              else state.selected.add(s.id);
              persistFilters();
              render();
            },
            [
              el('span', { class: 'nm', title: s.name }, s.name, s.requires_login ? el('small', { text: t.requiresLogin }) : null),
              el('span', { class: 'n', text: state.searchedSites.has(s.id) ? base.filter((l) => l.site_id === s.id).length : '' }),
            ],
          ),
        )
      : [el('div', { class: 'none', text: t.noStoreFound })]),
  );
}

function buildStores(pop: HTMLElement) {
  const find = el('input', {
    type: 'text',
    placeholder: t.findStore,
    'aria-label': t.findStore,
    value: storeQ,
    spellcheck: 'false',
    oninput: (e: Event) => {
      storeQ = (e.target as HTMLInputElement).value;
      paintStoreList();
    },
  });
  storeList = el('div');
  pop.append(
    el('label', { class: 'find' }, icon('i-search'), find),
    el(
      'div',
      { class: 'hd' },
      el('button', {
        type: 'button',
        text: t.selectAll,
        onclick: () => {
          state.selected = new Set(state.sites.map((s) => s.id));
          persistFilters();
          render();
        },
      }),
      el('button', {
        type: 'button',
        text: t.restoreDefault,
        onclick: () => {
          state.selected = new Set(state.sites.filter((s) => !s.requires_login && !['olx', 'enjoei'].includes(s.id)).map((s) => s.id));
          persistFilters();
          render();
        },
      }),
    ),
    storeList,
  );
  paintStoreList();
}

function renderStores() {
  const box = $('#stores');
  let btn = box.querySelector<HTMLButtonElement>(':scope > button');
  if (!btn) {
    btn = el('button', { type: 'button', 'aria-haspopup': 'listbox', 'aria-expanded': 'false' });
    const b = btn;
    b.addEventListener('click', (e) => {
      e.stopPropagation();
      togglePop(box, b, buildStores, true);
    });
    box.append(b);
  }
  btn.replaceChildren(el('span', { class: 'val', text: storesSummary() }), icon('i-down'));
  if (box.classList.contains('open')) paintStoreList();
}

function bounds(): [number, number] {
  if (!state.listings.length) return [0, 10000];
  const ps = state.listings.map((l) => l.price);
  return [Math.floor(Math.min(...ps) / 50) * 50, Math.ceil(Math.max(...ps) / 50) * 50];
}

const fmt = (v: number) => Math.round(v).toLocaleString('pt-BR');

function renderPrice() {
  const [lo, hi] = bounds();
  const base = state.listings.filter((l) => inIntl(l) && inText(l) && inCond(l.condition) && inSite(l.site_id));
  const N = 22;
  const bins = new Array(N).fill(0);
  for (const l of base) bins[Math.min(N - 1, Math.floor(((l.price - lo) / (hi - lo || 1)) * N))]++;
  const max = Math.max(1, ...bins);
  const a = state.lo ?? lo;
  const b = state.hi ?? hi;
  $('#hist').replaceChildren(
    ...bins.map((c, i) => {
      const mid = lo + ((i + 0.5) / N) * (hi - lo);
      return el('i', { class: mid >= a && mid <= b ? 'in' : '', style: `height:${c ? 8 + (c / max) * 92 : 3}%` });
    }),
  );
  for (const [id, v] of [['#r-lo', a], ['#r-hi', b]] as const) {
    const r = $<HTMLInputElement>(id);
    r.min = String(lo);
    r.max = String(hi);
    r.step = '10';
    r.value = String(v);
  }
  const has = state.listings.length > 0;
  $('#hist').hidden = !has;
  const pLo = $<HTMLInputElement>('#p-lo');
  const pHi = $<HTMLInputElement>('#p-hi');
  if (document.activeElement !== pLo) pLo.value = state.lo == null ? '' : fmt(state.lo);
  if (document.activeElement !== pHi) pHi.value = state.hi == null ? '' : fmt(state.hi);
  pLo.placeholder = has ? fmt(lo) : '0';
  pHi.placeholder = has ? fmt(hi) : '';
  pHi.closest('label')?.classList.toggle('inf', !has && state.hi == null);
}

function setRange(lo: number | null, hi: number | null, fromSlider = false) {
  const [a, b] = bounds();
  state.lo = lo == null || lo <= a ? null : lo;
  state.hi = hi == null || (fromSlider && hi >= b) ? null : hi;
  if (state.lo != null && state.hi != null && state.lo > state.hi) [state.lo, state.hi] = [state.hi, state.lo];
  render();
}

const num = (v: string) => {
  const n = parseFloat(v.replace(/\./g, '').replace(',', '.'));
  return Number.isFinite(n) ? n : null;
};

/* ---------- plan, notices, results ---------- */

function renderPlan() {
  const p = state.plan;
  const box = $('#plan');
  if (!p) return box.replaceChildren();
  const tok = (kids: (Node | string)[], onRemove: () => void) =>
    el('span', { class: 'tok' }, ...kids, el('button', { type: 'button', class: 'x', title: t.removeFromSearch, onclick: onRemove }, icon('i-x')));
  const kids: Node[] = [el('span', { class: 'tok kind', text: p.product })];
  if (p.alternatives.length) {
    kids.push(
      tok(
        p.alternatives.flatMap((a, i) => (i ? [el('span', { class: 'or', text: t.or }), a] : [a])),
        () => editPlan((x) => (x.alternatives = [])),
      ),
    );
  }
  p.requirements.forEach((r, i) => kids.push(tok([r], () => editPlan((x) => x.requirements.splice(i, 1)))));
  box.replaceChildren(...kids);
}

function editPlan(f: (p: SearchPlan) => void) {
  if (!state.plan) return;
  f(state.plan);
  state.planDirty = true;
  renderPlan();
  render();
}

const joinNames = (xs: string[]) => (xs.length < 2 ? xs.join('') : `${xs.slice(0, -1).join(', ')} ${t.and} ${xs[xs.length - 1]}`);

function renderMore() {
  const box = $('#more');
  if (!searched() || isRunning()) return (box.hidden = true);
  if (state.planDirty) {
    box.hidden = false;
    box.replaceChildren(el('p', { text: t.planChanged }), el('button', { class: 'btn', onclick: () => startNew(true) }, icon('i-refresh'), t.searchAgain));
    return;
  }
  const sites = [...state.selected].filter((id) => !state.searchedSites.has(id)).map((id) => siteById(id)?.name ?? id);
  const conds = [...state.cond].filter((c) => !state.searchedCond.has(c));
  if (!sites.length && !conds.length) return (box.hidden = true);
  const items: string[] = [];
  if (conds.length) items.push(conds[0] === 'used' ? t.usedProducts : t.newProducts);
  const shown = sites.slice(0, 4);
  items.push(...shown);
  if (sites.length > shown.length) items.push(t.andMore(sites.length - shown.length));
  box.hidden = false;
  box.replaceChildren(
    el('p', { text: t.missing(joinNames(items)) }),
    el(
      'div',
      { class: 'acts' },
      el('button', { class: 'btn', onclick: () => startMissing() }, icon('i-plus'), t.searchMissing),
      el('button', { class: 'btn', onclick: () => startNew(true) }, icon('i-refresh'), t.searchAllAgain),
    ),
  );
}

const brl = (v: number) => v.toLocaleString('pt-BR', { style: 'currency', currency: 'BRL' });

function price(v: number, original?: number) {
  const c = Math.round((v % 1) * 100);
  const was =
    original && original > v
      ? el(
          'span',
          { class: 'was', role: 'tooltip' },
          el('s', { text: brl(original) }),
          el('span', { class: 'off', text: `-${Math.round((1 - v / original) * 100)}%` }),
        )
      : null;
  return el(
    'span',
    { class: 'price' + (was ? ' has-was' : ''), 'aria-label': was ? `${brl(v)}. ${t.wasPrice}: ${brl(original!)}` : brl(v) },
    el('span', { class: 'cur', text: 'R$' }),
    el('span', { class: 'int', text: Math.floor(v).toLocaleString('pt-BR') }),
    el('span', { class: 'ct', text: ',' + String(c).padStart(2, '0') }),
    was,
  );
}

function openListing(url: string) {
  if (!/^https?:\/\//i.test(url)) return;
  api.openUrl(url).catch((e) => toast(errText(e)));
}

function renderList() {
  const all = visible();
  const bn = all.find((l) => l.condition === 'new');
  const bu = all.find((l) => l.condition === 'used');
  $('#list').replaceChildren(
    ...all.map((l) => {
      const li = el(
        'li',
        { class: 'row' + (l === bn || l === bu ? ' best' : '') + (l.fresh ? ' fresh' : '') },
        el('span', { class: 'dot ' + (l.condition === 'used' ? 'u' : 'f'), title: l.condition === 'used' ? t.used : t.new }),
        price(l.price, l.original_price),
        el(
          'div',
          { class: 't' },
          el('div', { class: 'ti' }, el('span', { class: 'tt', text: l.title, title: l.title }), l.international ? el('span', { class: 'intl', text: t.intlBadge, title: t.intlTip }) : null),
          el('div', { class: 'sub', text: l.site_name }),
        ),
        el('button', { class: 'dest', title: t.openListing(l.site_name), onclick: () => openListing(l.url) }, icon('i-out'), el('span', { text: l.site_name })),
      );
      l.fresh = false;
      return li;
    }),
  );
  const n = all.length;
  $('#count').textContent = t.results(n) + (state.elapsed ? ` · ${state.elapsed}` : '');
  $('#rh').hidden = !searched() && !isRunning();
  $('#empty').hidden = searched() || isRunning();
}

function renderAgents() {
  const box = $('#agents');
  const list = [...state.agents.values()];
  $('#t-progress').setAttribute('aria-checked', String(state.progress));
  box.hidden = !state.progress || !list.length;
  if (box.hidden) return;
  box.replaceChildren(
    ...list.map((a) =>
      el(
        'div',
        { class: 'agent ' + a.st },
        el('span', { class: 'nm', text: a.label }),
        el('span', { class: 'trk' }, el('i', { style: `width:${a.pct}%` })),
        el('span', { class: 'm', text: a.msg, title: a.msg }),
        el('span', { class: 'st' }, a.st === 'ok' ? icon('i-check') : a.st === 'fail' ? icon('i-x') : null),
      ),
    ),
  );
}

function renderHuman() {
  const box = $('#human');
  const req = [...state.human.values()].at(-1);
  if (!req) return (box.hidden = true);
  const site = (req.site && siteById(req.site)?.name) || t.aStore;
  const text =
    req.kind === 'login' ? t.humanLogin(site) : req.kind === 'expired' ? t.humanExpired(site) : req.kind === 'challenge' ? t.humanChallenge(site) : t.humanOther(req.reason);
  const resolve = (ok: boolean) => {
    state.human.delete(req.agent);
    renderHuman();
    api.resolveHuman(req.agent, ok);
  };
  box.hidden = false;
  box.replaceChildren(
    el('p', { text }),
    el('button', { class: 'btn pri', onclick: () => resolve(true) }, icon('i-check'), t.done),
    el('button', { class: 'btn', onclick: () => resolve(false) }, icon('i-skip'), t.skipStore),
  );
}

export function render() {
  renderCond();
  renderOrigin();
  renderStores();
  renderPrice();
  renderMore();
  renderList();
  renderAgents();
}

/* ---------- running searches ---------- */

function setRunningUi() {
  const on = isRunning();
  $('#app').classList.toggle('running', on);
  $('#go').replaceChildren(icon(on ? 'i-stop' : 'i-search'));
  $('#go').title = on ? t.stopButton : t.searchButton;
}

async function launch(req: SearchRequest, main: boolean) {
  try {
    const job = await api.startSearch(req);
    state.jobs.set(job, { sites: req.sites, conds: condsOf(req.condition), main });
    state.running.add(job);
    return true;
  } catch (e) {
    toast(errText(e));
    return false;
  }
}

async function startNew(keepPlan = false) {
  const query = $<HTMLInputElement>('#q').value.trim();
  if (!query) return;
  if (!state.selected.size) return toast(t.selectStore);
  const plan = keepPlan ? state.plan : null;
  await stopAll(true);
  Object.assign(state, {
    query,
    plan,
    planDirty: false,
    listings: [],
    elapsed: null,
    startedAt: Date.now(),
  });
  state.keys.clear();
  state.searchedSites.clear();
  state.searchedCond.clear();
  state.failedSites.clear();
  state.agents.clear();
  state.human.clear();
  renderPlan();
  renderHuman();
  const ok = await launch({ query, condition: condFilter(state.cond), sites: [...state.selected], mode: null, max_price: state.hi, plan }, true);
  setRunningUi();
  if (ok) render();
}

async function startMissing() {
  const newSites = [...state.selected].filter((id) => !state.searchedSites.has(id));
  const oldSites = [...state.selected].filter((id) => state.searchedSites.has(id));
  const missing = [...state.cond].filter((c) => !state.searchedCond.has(c));
  const base = { query: state.query, mode: null, max_price: null, plan: state.plan };
  state.startedAt = Date.now();
  state.elapsed = null;
  if (newSites.length) await launch({ ...base, sites: newSites, condition: condFilter(state.cond) }, false);
  if (missing.length && oldSites.length) await launch({ ...base, sites: oldSites, condition: condFilter(missing) }, false);
  setRunningUi();
  render();
}

let stopping: Promise<void> | null = null;

function stopAll(silent = false): Promise<void> {
  if (stopping) return stopping;
  if (!isRunning()) return Promise.resolve();
  if (!silent) toast(t.searchCancelled);
  stopping = api
    .cancelAll()
    .catch((e) => toast(errText(e)))
    .finally(() => {
      stopping = null;
    });
  return stopping;
}

function onEvent(ev: SearchEvent) {
  const meta = state.jobs.get(ev.job);
  if (!meta) return;
  switch (ev.type) {
    case 'started':
      for (const a of ev.agents) state.agents.set(a.id, { job: ev.job, label: a.label, sites: a.sites, st: 'wait', pct: 0, msg: t.queued });
      renderAgents();
      break;
    case 'planned':
      if (meta.main) {
        state.plan = ev.plan;
        if (ev.plan.condition) {
          meta.conds = condsOf(ev.plan.condition);
          state.cond = new Set(meta.conds);
        }
        if (ev.plan.max_price) state.hi = ev.plan.max_price;
        renderPlan();
        render();
      } else if (ev.plan.condition) {
        meta.conds = condsOf(ev.plan.condition);
      }
      break;
    case 'log': {
      const a = state.agents.get(ev.agent);
      if (a) {
        a.st = 'run';
        a.msg = ev.message;
        a.pct = Math.min(92, a.pct + (92 - a.pct) * 0.12);
      }
      renderAgents();
      break;
    }
    case 'listing': {
      const l = ev.listing;
      const key = l.url.split(/[?#]/)[0].replace(/\/$/, '').toLowerCase() + '|' + l.title.toLowerCase().replace(/\s+/g, ' ');
      if (state.keys.has(key)) break;
      state.keys.add(key);
      state.listings.push({ ...l, fresh: true });
      render();
      break;
    }
    case 'needs_human': {
      state.human.set(ev.agent, { agent: ev.agent, site: ev.site, kind: ev.kind, reason: ev.reason });
      const a = state.agents.get(ev.agent);
      if (a) {
        a.st = 'wait';
        a.msg = t.waitingLogin;
      }
      renderHuman();
      renderAgents();
      break;
    }
    case 'human_resolved':
      state.human.delete(ev.agent);
      renderHuman();
      break;
    case 'agent_finished': {
      const a = state.agents.get(ev.agent);
      if (a) {
        a.st = ev.ok ? 'ok' : 'fail';
        a.pct = ev.ok ? 100 : a.pct;
        a.msg = ev.summary || (ev.ok ? t.doneState : t.failedState);
        if (!ev.ok) a.sites.forEach((s) => state.failedSites.add(s));
        else {
          a.sites.forEach((s) => state.searchedSites.add(s));
          meta.conds.forEach((c) => state.searchedCond.add(c));
          renderMore();
        }
      }
      renderAgents();
      break;
    }
    case 'finished': {
      state.running.delete(ev.job);
      if (!ev.cancelled) {
        meta.sites.filter((s) => !state.failedSites.has(s)).forEach((s) => state.searchedSites.add(s));
        meta.conds.forEach((c) => state.searchedCond.add(c));
      }
      const failed = [...state.agents.values()].filter((a) => a.job === ev.job && a.st === 'fail');
      const total = [...state.agents.values()].filter((a) => a.job === ev.job).length;
      if (total > 0 && failed.length === total && !ev.cancelled) toast(failed[0].msg);
      if (!isRunning()) {
        state.elapsed = ev.cancelled ? t.cancelled : t.seconds(Math.round((Date.now() - state.startedAt) / 1000));
        state.human.clear();
        renderHuman();
      }
      setRunningUi();
      render();
      break;
    }
  }
}

export function initSearch() {
  $('#form').addEventListener('submit', (e) => {
    e.preventDefault();
    if (isRunning()) return stopAll();
    startNew();
  });
  const filter = $<HTMLInputElement>('#r-filter');
  filter.addEventListener('input', () => {
    textWords = fold(filter.value).split(/\s+/).filter(Boolean);
    render();
  });
  filter.addEventListener('keydown', (e) => {
    if (e.key === 'Escape' && filter.value) {
      e.stopPropagation();
      filter.value = '';
      textWords = [];
      render();
    }
  });
  $('#t-progress').addEventListener('click', () => {
    state.progress = !state.progress;
    renderAgents();
  });
  $('#r-lo').addEventListener('input', (e) => setRange(+(e.target as HTMLInputElement).value, state.hi, true));
  $('#r-hi').addEventListener('input', (e) => setRange(state.lo, +(e.target as HTMLInputElement).value, true));
  $('#p-lo').addEventListener('change', (e) => setRange(num((e.target as HTMLInputElement).value), state.hi));
  $('#p-hi').addEventListener('change', (e) => setRange(state.lo, num((e.target as HTMLInputElement).value)));
  document.addEventListener('keydown', (e) => {
    if (e.key === 'Escape' && !$$('.sheet, .modal').some((x) => !x.hidden) && !$('#stores').classList.contains('open')) stopAll();
  });
  api.onEvent(onEvent);
  setRunningUi();
}

export const closeStoresPop = () => closePop();
