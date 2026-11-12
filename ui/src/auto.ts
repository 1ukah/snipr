import { api } from './api';
import type { Every } from './bindings/Every';
import type { Schedule } from './bindings/Schedule';
import type { ScheduleEvent } from './bindings/ScheduleEvent';
import type { ScheduleResults } from './bindings/ScheduleResults';
import { $, check, el, errText, icon, toast } from './dom';
import { addTotalUsage } from './footer';
import { segment } from './popover';
import { columns, listingRow } from './rows';
import { type Cond, condFilter, condsOf, state } from './state';
import { storePicker } from './storepick';
import { t } from './strings';

let list: Schedule[] = [];
/** Schedule being edited; null = a new one. */
let selected: string | null = null;
let draft: Schedule = blank();
let results: ScheduleResults | null = null;

function blank(): Schedule {
  return {
    id: '',
    query: '',
    sites: [...state.selected],
    condition: condFilter(state.cond),
    max_price: null,
    every: { kind: 'interval', minutes: 60 },
    enabled: true,
    plan: null,
    last_run: null,
    last_total: 0,
    last_new: 0,
    last_error: null,
    running: false,
    next_run: null,
  };
}

const byId = (id: string | null) => list.find((s) => s.id === id);
const pad = (n: number) => String(n).padStart(2, '0');
const clock = (secs: number) => {
  const d = new Date(secs * 1000);
  const time = `${pad(d.getHours())}:${pad(d.getMinutes())}`;
  return d.toDateString() === new Date().toDateString() ? time : `${pad(d.getDate())}/${pad(d.getMonth() + 1)} ${time}`;
};
const everyText = (e: Every) => (e.kind === 'interval' ? t.everyMinutes(e.minutes) : t.dailyAt(`${pad(e.hour)}:${pad(e.minute)}`));

function status(s: Schedule | undefined): string {
  if (!s) return '';
  if (s.running) return t.autoRunning;
  const parts: string[] = [];
  if (s.last_error) parts.push(t.autoFailed(s.last_error));
  else if (s.last_run) parts.push(t.autoLast(clock(s.last_run), s.last_total, s.last_new));
  if (!s.enabled) parts.push(t.autoPaused);
  else if (s.next_run) parts.push(t.autoNext(clock(s.next_run)));
  return parts.join(' · ');
}

/* ---------- side list ---------- */

function renderSide() {
  const box = $('#a-side');
  box.replaceChildren(
    el('div', { class: 'fl' }, icon('i-clock'), el('span', { text: t.tabAuto }), el('span', { class: 'n', text: list.length || '' })),
    el('button', { type: 'button', class: 'btn a-new' + (selected === null ? ' on' : ''), onclick: () => pick(null) }, icon('i-plus'), t.newAuto),
    ...list.map((s) =>
      el(
        'button',
        { type: 'button', class: 'a-item' + (s.id === selected ? ' on' : '') + (s.enabled ? '' : ' off'), onclick: () => pick(s.id) },
        el('span', { class: 'nm', text: s.query, title: s.query }),
        s.running ? el('span', { class: 'live', title: t.autoRunning }) : null,
        el('span', { class: 'sub', text: [everyText(s.every), s.last_run ? t.resultsShort(s.last_total, s.last_new) : null].filter(Boolean).join(' · ') }),
      ),
    ),
  );
  $('#tab-auto-live').hidden = !list.some((s) => s.running);
}

/* ---------- editor ---------- */

function field(cls: string, pre: string | null, input: HTMLInputElement, post?: string) {
  return el('label', { class: 'field ' + cls }, pre ? el('span', { class: 'pre', text: pre }) : null, input, post ? el('span', { class: 'pre', text: post }) : null);
}

function numIn(value: number | null, label: string, onChange: (n: number | null) => void, placeholder = '') {
  return el('input', {
    inputmode: 'numeric',
    value: value == null ? '' : String(value),
    placeholder,
    'aria-label': label,
    oninput: (e: Event) => {
      const v = (e.target as HTMLInputElement).value.replace(/\./g, '').replace(',', '.').trim();
      const n = parseFloat(v);
      onChange(v && Number.isFinite(n) ? n : null);
    },
  });
}

function renderEveryInputs(box: HTMLElement) {
  const e = draft.every;
  if (e.kind === 'interval') {
    box.replaceChildren(
      field('a-num', null, numIn(e.minutes, t.minutes, (n) => (draft.every = { kind: 'interval', minutes: Math.round(n ?? 0) })), t.minutesShort),
    );
  } else {
    const hour = numIn(e.hour, t.hour, (n) => (draft.every = { ...(draft.every as Extract<Every, { kind: 'daily' }>), hour: Math.round(n ?? -1) }), '09');
    const minute = numIn(e.minute, t.minute, (n) => (draft.every = { ...(draft.every as Extract<Every, { kind: 'daily' }>), minute: Math.round(n ?? -1) }), '00');
    hour.value = pad(e.hour);
    minute.value = pad(e.minute);
    box.replaceChildren(el('label', { class: 'field a-time' }, hour, el('span', { class: 'pre', text: ':' }), minute));
  }
}

function renderEditor() {
  const box = $('#a-edit');
  const saved = byId(selected);
  const q = el('input', {
    value: draft.query,
    placeholder: t.searchPlaceholder,
    'aria-label': t.search,
    spellcheck: 'false',
    oninput: (e: Event) => (draft.query = (e.target as HTMLInputElement).value),
    onkeydown: (e: KeyboardEvent) => e.key === 'Enter' && save(false),
  });
  const stores = el('div', { class: 'sel a-stores' });
  const picker = storePicker(stores, {
    selected: () => new Set(draft.sites),
    onChange: (next) => {
      draft.sites = [...next];
      picker.paint();
    },
  });
  const conds = el('div', { class: 'a-cond' });
  const paintConds = () =>
    conds.replaceChildren(
      ...(['new', 'used'] as Cond[]).map((k) => {
        const cur = new Set(condsOf(draft.condition));
        return check(
          cur.has(k),
          () => {
            if (cur.has(k)) cur.delete(k);
            else cur.add(k);
            if (!cur.size) cur.add(k);
            draft.condition = condFilter(cur);
            paintConds();
          },
          [el('span', { class: 'nm', text: k === 'new' ? t.new : t.used })],
        );
      }),
    );
  paintConds();
  const every = el('div', { class: 'seg a-every' });
  const everyIn = el('div', { class: 'a-every-in' });
  segment(
    every,
    [
      ['interval', t.everyLabel, 'i-refresh'],
      ['daily', t.dailyLabel, 'i-clock'],
    ],
    draft.every.kind,
    (v) => {
      if (v === draft.every.kind) return;
      draft.every = v === 'interval' ? { kind: 'interval', minutes: 60 } : { kind: 'daily', hour: 9, minute: 0 };
      renderEveryInputs(everyIn);
    },
  );
  renderEveryInputs(everyIn);
  const enabled = el('button', {
    type: 'button',
    class: 'tg',
    role: 'switch',
    'aria-checked': String(draft.enabled),
    onclick: () => {
      draft.enabled = !draft.enabled;
      enabled.setAttribute('aria-checked', String(draft.enabled));
    },
  });
  enabled.append(el('span', { class: 'sw' }), el('span', { text: t.autoEnabled }));

  const running = !!saved?.running;
  box.replaceChildren(
    ...(selected === null && !list.length ? [el('p', { class: 'why', text: t.autoHelp })] : []),
    el('div', { class: 'bar a-q' }, q),
    el(
      'div',
      { class: 'a-row' },
      stores,
      conds,
      field('a-price', t.upTo, numIn(draft.max_price, t.priceMax, (n) => (draft.max_price = n))),
    ),
    el('div', { class: 'a-row' }, every, everyIn, enabled),
    el(
      'div',
      { class: 'a-acts' },
      el('button', { type: 'button', class: 'btn pri', onclick: () => save(false) }, icon('i-check'), selected ? t.saveChanges : t.createAuto),
      running
        ? el('button', { type: 'button', class: 'btn', onclick: () => api.stopSchedule(selected!) }, icon('i-stop'), t.stop)
        : el('button', { type: 'button', class: 'btn', onclick: () => save(true) }, icon('i-search'), t.runNow),
      selected ? el('button', { type: 'button', class: 'btn', onclick: remove }, icon('i-trash'), t.remove) : null,
      el('span', { class: 'a-status' + (saved?.last_error ? ' err' : ''), text: status(saved) }),
    ),
  );
  if (selected === null) setTimeout(() => q.focus(), 0);
}

function renderResults() {
  const head = $('#a-rh');
  const box = $('#a-list');
  const cols = $('#a-cols');
  if (!selected || !results?.at) {
    head.hidden = true;
    box.replaceChildren();
    cols.replaceChildren();
    return;
  }
  const fresh = new Set(results.new);
  const all = [...results.listings].sort((a, b) => a.price - b.price);
  const bn = all.find((l) => l.condition === 'new');
  const bu = all.find((l) => l.condition === 'used');
  head.hidden = false;
  head.replaceChildren(el('span', { text: t.results(all.length) + (fresh.size ? ` · ${t.recentCount(fresh.size)}` : '') + ` · ${clock(results.at)}` }));
  cols.replaceChildren(...(all.length ? [columns()] : []));
  box.replaceChildren(...all.map((l) => listingRow(l, { best: l === bn || l === bu, recent: fresh.has(l.id) })));
}

export function renderAuto() {
  renderSide();
  renderEditor();
  renderResults();
}

async function loadResults(id: string) {
  const r = await api.scheduleResults(id).catch(() => null);
  if (selected !== id) return;
  results = r;
  renderResults();
}

function pick(id: string | null) {
  selected = id;
  const s = byId(id);
  draft = s ? structuredClone(s) : blank();
  results = null;
  renderAuto();
  if (id) loadResults(id);
}

function upsert(s: Schedule) {
  const i = list.findIndex((x) => x.id === s.id);
  if (i >= 0) list[i] = s;
  else list.push(s);
}

async function save(run: boolean) {
  const e = draft.every;
  if (e.kind === 'daily' && (e.hour < 0 || e.hour > 23 || e.minute < 0 || e.minute > 59)) return toast(t.invalidTime);
  try {
    const s = await api.saveSchedule({ ...draft, query: draft.query.trim() });
    upsert(s);
    selected = s.id;
    draft = structuredClone(s);
    if (run) await api.runSchedule(s.id);
    else toast(t.autoSaved);
    renderAuto();
    loadResults(s.id);
  } catch (err) {
    toast(errText(err));
  }
}

async function remove() {
  if (!selected) return;
  try {
    await api.deleteSchedule(selected);
    list = list.filter((s) => s.id !== selected);
    pick(null);
  } catch (err) {
    toast(errText(err));
  }
}

function onAuto(ev: ScheduleEvent) {
  if (ev.type === 'usage') return addTotalUsage(ev.usage);
  if (ev.type === 'removed') {
    list = list.filter((s) => s.id !== ev.id);
    if (selected === ev.id) pick(null);
    else renderSide();
    return;
  }
  const s = ev.schedule;
  const was = byId(s.id);
  upsert(s);
  const finished = was?.running && !s.running;
  if (finished && s.last_new > 0) toast(t.autoNew(s.query, s.last_new));
  if (state.tab !== 'auto') return renderSide();
  if (selected === s.id) {
    // Only the status changes: the fields being edited stay as they are.
    Object.assign(draft, { running: s.running, last_run: s.last_run, last_total: s.last_total, last_new: s.last_new, last_error: s.last_error, next_run: s.next_run, plan: s.plan });
    renderEditor();
    if (finished) loadResults(s.id);
  }
  renderSide();
}

export async function initAuto() {
  list = await api.schedules().catch(() => []);
  api.onAuto(onAuto);
  renderSide();
}

export function openAuto() {
  if (selected && !byId(selected)) selected = null;
  if (selected === null && list.length) return pick(list[0].id);
  renderAuto();
  if (selected) loadResults(selected);
}
