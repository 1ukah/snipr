import type { Usage } from './bindings/Usage';
import { $, el, icon } from './dom';
import { type AgentView, state } from './state';
import { t } from './strings';

interface Tally {
  input: number;
  output: number;
  cost: number;
  priced: number;
  unpriced: number;
}

interface Totals extends Tally {
  searches: number;
}

/** One run: the manual search, or the last run of an automatic search. */
interface Run {
  tally: Tally;
  model: string | null;
  startedAt: number;
  /** Set when the run ends. */
  secs: number | null;
}

type Kind = 'manual' | 'auto';

const KEY: Record<Kind, string> = { manual: 'snipr.usage', auto: 'snipr.usage.auto' };
const blank = (): Tally => ({ input: 0, output: 0, cost: 0, priced: 0, unpriced: 0 });
const newRun = (): Run => ({ tally: blank(), model: null, startedAt: Date.now(), secs: null });

let manual = newRun();
const autoRuns = new Map<string, Run>();
const totals: Record<Kind, Totals> = { manual: load('manual'), auto: load('auto') };

/** What the automatic tab shows: the selected search and its store progress. */
let autoView: () => { id: string | null; running: boolean; agents: AgentView[] } = () => ({ id: null, running: false, agents: [] });
export const bindAutoView = (f: typeof autoView) => (autoView = f);

function load(kind: Kind): Totals {
  try {
    const v = JSON.parse(localStorage.getItem(KEY[kind]) ?? '');
    if (v && typeof v.searches === 'number') return { ...blank(), ...v };
  } catch {}
  return { ...blank(), searches: 0 };
}

function persist(kind: Kind) {
  try {
    localStorage.setItem(KEY[kind], JSON.stringify(totals[kind]));
  } catch {}
}

function add(x: Tally, u: Usage) {
  x.input += u.input_tokens;
  x.output += u.output_tokens;
  if (u.cost_usd == null) x.unpriced++;
  else {
    x.cost += u.cost_usd;
    x.priced++;
  }
}

export function countSearch() {
  manual = newRun();
  totals.manual.searches++;
  persist('manual');
  renderFooter();
}

export function addUsage(u: Usage) {
  add(manual.tally, u);
  add(totals.manual, u);
  if (u.model) manual.model = u.model;
  persist('manual');
  renderFooter();
}

export function autoRunStarted(id: string) {
  autoRuns.set(id, newRun());
  totals.auto.searches++;
  persist('auto');
  renderFooter();
}

export function autoRunFinished(id: string) {
  const r = autoRuns.get(id);
  if (r && r.secs == null) r.secs = Math.round((Date.now() - r.startedAt) / 1000);
  renderFooter();
}

export function addAutoUsage(id: string, u: Usage) {
  let r = autoRuns.get(id);
  if (!r) autoRuns.set(id, (r = newRun()));
  add(r.tally, u);
  if (u.model) r.model = u.model;
  add(totals.auto, u);
  persist('auto');
  renderFooter();
}

function resetTotals() {
  const kind: Kind = state.tab === 'auto' ? 'auto' : 'manual';
  totals[kind] = { ...blank(), searches: 0 };
  persist(kind);
  renderFooter();
}

const compact = new Intl.NumberFormat('pt-BR', { notation: 'compact', maximumFractionDigits: 1 });
const full = (n: number) => n.toLocaleString('pt-BR');
const tokens = (n: number) => t.tokens(compact.format(n));
const usd = (v: number) => v.toLocaleString('pt-BR', { style: 'currency', currency: 'USD', minimumFractionDigits: 2, maximumFractionDigits: v < 1 ? 3 : 2 });

function cost(x: Tally) {
  if (!x.priced) return null;
  return (x.unpriced ? '≥ ' : '') + usd(x.cost);
}

const tokensTitle = (x: Tally) => `${t.inputTokens}: ${full(x.input)} · ${t.outputTokens}: ${full(x.output)}`;
const costTitle = (x: Tally) => (!x.priced ? t.costUnknown : x.unpriced ? t.costPartial : '');

function item(ic: string, label: string, parts: (string | null | false)[], title = '') {
  return el(
    'span',
    { class: 'fi', title: title || null },
    icon(ic),
    el('span', { class: 'k', text: label }),
    el('span', { class: 'v', text: parts.filter(Boolean).join(' · ') }),
  );
}

const CLI: Record<string, string> = { anthropic: 'Claude Code', openai: 'Codex', google: 'Antigravity' };

function engine() {
  const ai = state.settings.ai;
  if (ai.mode === 'login') return CLI[ai.login_provider] ?? ai.login_provider;
  if (ai.mode === 'oauth') return 'Claude Code (OAuth)';
  return `${t.providerName[ai.key_provider]} API`;
}

/** The run the current tab shows. */
function shown(): { run: Run | null; agents: AgentView[]; running: boolean; label: string; kind: Kind } {
  if (state.tab === 'auto') {
    const v = autoView();
    return { run: v.id ? (autoRuns.get(v.id) ?? null) : null, agents: v.agents, running: v.running, label: t.autoRun, kind: 'auto' };
  }
  return { run: manual, agents: [...state.agents.values()], running: state.running.size > 0, label: t.search, kind: 'manual' };
}

function config(model: string | null) {
  const s = state.settings;
  const ai = s.ai;
  const browser: Record<string, string> = { hidden: t.hidden, headless: t.headless, visible: t.visible };
  return el(
    'div',
    { class: 'cfg' },
    item('i-spark', t.ai, [engine(), model || ai.fast_model || t.automatic, ai.effort && t.effort[ai.effort]]),
    item('i-bot', t.agents, [t.steps(s.max_steps)]),
    item('i-globe', t.browser, [browser[s.browser_mode], t.tabs(s.max_parallel)]),
  );
}

function elapsed(kind: Kind, run: Run, running: boolean) {
  if (kind === 'manual') return running && state.startedAt ? t.seconds(Math.round((Date.now() - state.startedAt) / 1000)) : state.elapsed;
  if (running) return t.seconds(Math.round((Date.now() - run.startedAt) / 1000));
  return run.secs == null ? null : t.seconds(run.secs);
}

function stats(v: ReturnType<typeof shown>) {
  const tot = totals[v.kind];
  const box = el('div', { class: 'stats' });
  const run = v.run;
  const done = v.agents.filter((a) => a.st === 'ok' || a.st === 'fail').length;
  if (run && (v.agents.length || run.tally.input || run.tally.output)) {
    box.append(
      item(
        'i-pulse',
        v.label,
        [tokens(run.tally.input + run.tally.output), cost(run.tally), v.agents.length > 0 && t.agentsDone(done, v.agents.length), elapsed(v.kind, run, v.running)],
        [tokensTitle(run.tally), costTitle(run.tally)].filter(Boolean).join('\n'),
      ),
    );
  }
  const avg = tot.searches ? (tot.input + tot.output) / tot.searches : 0;
  box.append(
    item(
      'i-chip',
      v.kind === 'auto' ? t.totalAuto : t.total,
      [v.kind === 'auto' ? t.runs(tot.searches) : t.searches(tot.searches), tot.searches > 0 && (v.kind === 'auto' ? t.avgPerRun : t.avgPerSearch)(tokens(Math.round(avg))), cost(tot)],
      [tokensTitle(tot), costTitle(tot)].filter(Boolean).join('\n'),
    ),
    el('button', { type: 'button', class: 'ib sm', title: t.resetTotals, 'aria-label': t.resetTotals, onclick: resetTotals }, icon('i-refresh')),
  );
  return box;
}

export function renderFooter() {
  const box = $('#foot');
  const on = !!state.settings?.status_bar;
  box.hidden = !on;
  $('#app').classList.toggle('has-foot', on);
  if (!on) return;
  const v = shown();
  box.replaceChildren(config(v.run?.model ?? null), stats(v));
}

export function initFooter() {
  window.setInterval(() => (state.tab === 'auto' ? autoView().running : state.running.size > 0) && renderFooter(), 1000);
}
