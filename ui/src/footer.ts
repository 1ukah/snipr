import type { Usage } from './bindings/Usage';
import { $, el, icon } from './dom';
import { state } from './state';
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

const KEY = 'snipr.usage';
const blank = (): Tally => ({ input: 0, output: 0, cost: 0, priced: 0, unpriced: 0 });

let current = blank();
let model: string | null = null;
let totals: Totals = load();

function load(): Totals {
  try {
    const v = JSON.parse(localStorage.getItem(KEY) ?? '');
    if (v && typeof v.searches === 'number') return { ...blank(), ...v };
  } catch {}
  return { ...blank(), searches: 0 };
}

function persist() {
  try {
    localStorage.setItem(KEY, JSON.stringify(totals));
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
  current = blank();
  model = null;
  totals.searches++;
  persist();
  renderFooter();
}

export function addUsage(u: Usage) {
  add(current, u);
  add(totals, u);
  if (u.model) model = u.model;
  persist();
  renderFooter();
}

function resetTotals() {
  totals = { ...blank(), searches: 0 };
  persist();
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

function config() {
  const s = state.settings;
  const ai = s.ai;
  const browser: Record<string, string> = { hidden: t.hidden, headless: t.headless, visible: t.visible };
  return el(
    'div',
    { class: 'cfg' },
    item('i-spark', t.ai, [engine(), model || ai.model || t.automatic, ai.effort && t.effort[ai.effort]]),
    item('i-bot', t.agents, [
      s.agent_mode === 'single' ? t.single : t.perStore,
      s.agent_mode === 'multi' && t.simultaneous(s.max_parallel),
      t.steps(s.max_steps),
    ]),
    item('i-globe', t.browser, [browser[s.browser_mode]]),
  );
}

function stats() {
  const agents = [...state.agents.values()];
  const done = agents.filter((a) => a.st === 'ok' || a.st === 'fail').length;
  const running = state.running.size > 0;
  const secs = running && state.startedAt ? t.seconds(Math.round((Date.now() - state.startedAt) / 1000)) : state.elapsed;
  const avg = totals.searches ? (totals.input + totals.output) / totals.searches : 0;
  const box = el('div', { class: 'stats' });
  if (agents.length || current.input || current.output) {
    box.append(
      item(
        'i-pulse',
        t.search,
        [tokens(current.input + current.output), cost(current), agents.length > 0 && t.agentsDone(done, agents.length), secs],
        [tokensTitle(current), costTitle(current)].filter(Boolean).join('\n'),
      ),
    );
  }
  box.append(
    item(
      'i-chip',
      t.total,
      [t.searches(totals.searches), totals.searches > 0 && t.avgPerSearch(tokens(Math.round(avg))), cost(totals)],
      [tokensTitle(totals), costTitle(totals)].filter(Boolean).join('\n'),
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
  box.replaceChildren(config(), stats());
}

export function initFooter() {
  window.setInterval(() => state.running.size && renderFooter(), 1000);
}
