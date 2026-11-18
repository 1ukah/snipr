import { api } from './api';
import type { ConditionFilter } from './bindings/ConditionFilter';
import type { HumanKind } from './bindings/HumanKind';
import type { Listing } from './bindings/Listing';
import type { SearchPlan } from './bindings/SearchPlan';
import type { Settings } from './bindings/Settings';
import type { Site } from './bindings/Site';
import type { Suggestion } from './bindings/Suggestion';
import { errText, toast } from './dom';

export type Cond = 'new' | 'used';

export const condFilter = (c: Iterable<Cond>): ConditionFilter => {
  const s = new Set(c);
  return s.has('new') && s.has('used') ? 'any' : s.has('used') ? 'used' : s.has('new') ? 'new' : 'any';
};
export const condsOf = (f: ConditionFilter): Cond[] => (f === 'any' ? ['new', 'used'] : [f]);

export interface AgentView {
  job: string;
  label: string;
  sites: string[];
  st: 'wait' | 'run' | 'ok' | 'fail';
  pct: number;
  msg: string;
}

export interface HumanRequest {
  agent: string;
  site: string | null;
  kind: HumanKind;
  reason: string;
}

export interface JobMeta {
  sites: string[];
  conds: Cond[];
  main: boolean;
}

export const state = {
  settings: null as unknown as Settings,
  sites: [] as Site[],
  suggestions: [] as Suggestion[],
  selected: new Set<string>(),
  cond: new Set<Cond>(['new']),
  query: '',
  plan: null as SearchPlan | null,
  planDirty: false,
  listings: [] as (Listing & { fresh?: boolean })[],
  keys: new Set<string>(),
  jobs: new Map<string, JobMeta>(),
  running: new Set<string>(),
  searchedSites: new Set<string>(),
  searchedCond: new Set<Cond>(),
  failedSites: new Set<string>(),
  agents: new Map<string, AgentView>(),
  human: new Map<string, HumanRequest>(),
  lo: null as number | null,
  hi: null as number | null,
  startedAt: 0,
  elapsed: null as string | null,
  progress: false,
  showIntl: false,
  /** Opened listing links (linkKey). */
  visited: new Set<string>(),
  tab: 'search' as 'search' | 'auto',
};

/** Same rule as `history::link_key` in the core. */
export const linkKey = (url: string) => url.split(/[?#]/)[0].replace(/\/+$/, '').toLowerCase();

export const siteById = (id: string) => state.sites.find((s) => s.id === id);

let saveTimer = 0;
export function saveSettings(patch: Partial<Settings>, now = false) {
  Object.assign(state.settings, patch);
  clearTimeout(saveTimer);
  const run = () => api.saveSettings(state.settings).catch((e) => toast(errText(e)));
  if (now) run();
  else saveTimer = window.setTimeout(run, 500);
}

export async function reloadSites() {
  const [settings, sites, suggestions] = await Promise.all([api.settings(), api.sites(), api.suggestions()]);
  state.settings = settings;
  state.sites = sites;
  state.suggestions = suggestions;
  state.selected = new Set(settings.enabled_sites.filter((id) => sites.some((s) => s.id === id)));
}

export function applyTheme(theme: Settings['theme']) {
  if (theme === 'system') document.documentElement.removeAttribute('data-theme');
  else document.documentElement.setAttribute('data-theme', theme);
}
