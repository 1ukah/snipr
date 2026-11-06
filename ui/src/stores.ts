import { api } from './api';
import type { Site } from './bindings/Site';
import { $, el, errText, fold, icon, toast } from './dom';
import { reloadSites, state } from './state';
import { t } from './strings';

interface Pending {
  key: string;
  name: string;
  domain: string;
  manual: boolean;
}

let q = '';
const pending: Pending[] = [];
const checking = new Set<string>();
let rechecking = false;
let onChange: () => void = () => {};

const domainOf = (url: string) =>
  url
    .trim()
    .replace(/^https?:\/\//i, '')
    .replace(/^www\./i, '')
    .split(/[?#]/)[0]
    .replace(/\/$/, '');

function card(name: string, domain: string, tag: { text: string; checking?: boolean } | null, action: { icon: string; title: string; run: () => void } | null, whole = false) {
  const kids = [
    el(
      'span',
      { class: 'tx' },
      el('span', { class: 'l1' }, el('span', { class: 'nm', text: name, title: name }), tag ? el('span', { class: 'tag' + (tag.checking ? ' chk' : ''), text: tag.text }) : null),
      el('span', { class: 'dm', text: domain, title: domain }),
    ),
  ];
  if (whole && action) return el('button', { type: 'button', class: 'card', title: action.title, onclick: action.run }, ...kids, el('span', { class: 'act' }, icon(action.icon)));
  return el('div', { class: 'card' }, ...kids, action ? el('button', { type: 'button', class: 'act', title: action.title, 'aria-label': action.title, onclick: action.run }, icon(action.icon)) : el('span'));
}

function siteCard(s: Site) {
  const tag = checking.has(s.id) ? { text: t.checkingLogin, checking: true } : s.requires_login ? { text: t.requiresLogin } : null;
  return card(s.name, domainOf(s.home_url || s.domain), tag, { icon: 'i-x', title: t.removeStore(s.name), run: () => remove(s) });
}

async function recheckAll() {
  if (rechecking) return;
  rechecking = true;
  const btn = $<HTMLButtonElement>('#sm-recheck');
  btn.disabled = true;
  btn.classList.add('busy');
  const queue = state.sites.map((s) => s.id);
  queue.forEach((id) => checking.add(id));
  renderStoresModal();
  const worker = async () => {
    for (let id = queue.shift(); id; id = queue.shift()) {
      try {
        const site = await api.checkSiteLogin(id);
        const i = state.sites.findIndex((s) => s.id === id);
        if (i >= 0) state.sites[i] = site;
      } catch (e) {
        toast(errText(e));
      }
      checking.delete(id);
      renderStoresModal();
    }
  };
  await Promise.all([worker(), worker(), worker()]);
  await reloadSites();
  rechecking = false;
  btn.disabled = false;
  btn.classList.remove('busy');
  renderStoresModal();
  onChange();
  toast(t.recheckDone);
}

export function renderStoresModal() {
  const f = fold(q.trim());
  const match = (name: string, domain: string) => !f || fold(name).includes(f) || fold(domain).includes(f);
  const checking = (manual: boolean) =>
    pending.filter((p) => p.manual === manual && match(p.name, p.domain)).map((p) => card(p.name, p.domain, { text: t.checkingLogin, checking: true }, null));
  const added = state.sites.filter((s) => !s.manual && match(s.name, s.domain));
  const manual = state.sites.filter((s) => s.manual && match(s.name, s.domain));
  const taken = (domain: string) => state.sites.some((x) => domain === x.domain || domain.startsWith(x.domain + '/') || domainOf(x.home_url) === domain);
  const sugg = state.suggestions.filter((s) => !pending.some((p) => p.key === s.url) && !taken(s.domain) && match(s.name, s.domain));
  const none = (text: string) => [el('div', { class: 'm-empty', text })];

  const addedCards = [...checking(false), ...[...added].reverse().filter((s) => s.custom), ...added.filter((s) => !s.custom)].map((x) => (x instanceof HTMLElement ? x : siteCard(x)));
  const manualCards = [...checking(true), ...[...manual].reverse().map(siteCard)];
  $('#sm-n-added').textContent = String(added.length + pending.filter((p) => !p.manual).length);
  $('#sm-n-sugg').textContent = String(sugg.length);
  $('#sm-n-man').textContent = String(manual.length + pending.filter((p) => p.manual).length);
  $('#sm-added').replaceChildren(...(addedCards.length ? addedCards : none(t.noStoreFound)));
  $('#sm-sugg').replaceChildren(
    ...(sugg.length
      ? sugg.map((s) => card(s.name, s.domain, null, { icon: 'i-plus', title: t.addStore(s.name), run: () => add(s.url, s.name, false) }, true))
      : none(t.noSuggestionFound)),
  );
  $('#sm-man').replaceChildren(...(manualCards.length ? manualCards : f ? none(t.noStoreFound) : []));
}

async function add(url: string, name: string | null, manual: boolean) {
  const p: Pending = { key: url, name: name ?? domainOf(url).split(/[./]/)[0], domain: domainOf(url), manual };
  pending.push(p);
  renderStoresModal();
  try {
    const site = await api.addSite(url, name, manual);
    await reloadSites();
    toast(site.requires_login ? t.storeNeedsLogin(site.name) : t.storeAdded(site.name));
  } catch (e) {
    toast(errText(e));
  }
  pending.splice(pending.indexOf(p), 1);
  renderStoresModal();
  onChange();
}

async function remove(s: Site) {
  try {
    await api.removeSite(s.id);
    await reloadSites();
  } catch (e) {
    toast(errText(e));
  }
  renderStoresModal();
  onChange();
}

export function openStoresModal() {
  q = '';
  $<HTMLInputElement>('#sm-q').value = '';
  $('#sm-url-msg').hidden = true;
  renderStoresModal();
}

export function initStores(changed: () => void) {
  onChange = changed;
  $('#sm-recheck').addEventListener('click', recheckAll);
  $('#sm-q').addEventListener('input', (e) => {
    q = (e.target as HTMLInputElement).value;
    renderStoresModal();
  });
  $('#sm-url').addEventListener('submit', (e) => {
    e.preventDefault();
    const input = $<HTMLInputElement>('#sm-url-in');
    const raw = input.value.trim();
    const msg = $('#sm-url-msg');
    let u: URL | null = null;
    try {
      u = new URL(/^https?:\/\//i.test(raw) ? raw : 'https://' + raw);
    } catch {
      u = null;
    }
    if (!u || !u.hostname.includes('.')) {
      msg.textContent = t.invalidUrl;
      msg.hidden = false;
      return;
    }
    msg.hidden = true;
    input.value = '';
    add(u.toString(), null, true);
  });
}
