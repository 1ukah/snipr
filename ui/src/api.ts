import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { openUrl } from '@tauri-apps/plugin-opener';
import type { BrowserStatus } from './bindings/BrowserStatus';
import type { CliStatus } from './bindings/CliStatus';
import type { Provider } from './bindings/Provider';
import type { Schedule } from './bindings/Schedule';
import type { ScheduleEvent } from './bindings/ScheduleEvent';
import type { ScheduleResults } from './bindings/ScheduleResults';
import type { SearchEvent } from './bindings/SearchEvent';
import type { SearchRequest } from './bindings/SearchRequest';
import type { Settings } from './bindings/Settings';
import type { Site } from './bindings/Site';
import type { Suggestion } from './bindings/Suggestion';

export const api = {
  settings: () => invoke<Settings>('get_settings'),
  saveSettings: (settings: Settings) => invoke<void>('save_settings', { settings }),
  sites: () => invoke<Site[]>('list_sites'),
  suggestions: () => invoke<Suggestion[]>('list_suggestions'),
  addSite: (url: string, name: string | null, manual: boolean) => invoke<Site>('add_site', { url, name, manual }),
  removeSite: (id: string) => invoke<void>('remove_site', { id }),
  checkSite: (id: string) => invoke<Site>('check_site', { id }),
  browserStatus: () => invoke<BrowserStatus>('browser_status'),
  testBrowser: () => invoke<string>('test_browser'),
  cliModels: (provider: Provider) => invoke<string[]>('cli_models', { provider }),
  secretStatus: (names: string[]) => invoke<Record<string, boolean>>('secret_status', { names }),
  setSecret: (name: string, value: string) => invoke<void>('set_secret', { name, value }),
  deleteSecret: (name: string) => invoke<void>('delete_secret', { name }),
  aiStatus: () => invoke<CliStatus[]>('ai_status'),
  openAiLogin: (provider: Provider) => invoke<void>('open_ai_login', { provider }),
  listModels: (provider: Provider, endpoint: string) => invoke<string[]>('list_models', { provider, endpoint }),
  startSearch: (req: SearchRequest) => invoke<string>('start_search', { req }),
  cancelSearch: (job: string) => invoke<void>('cancel_search', { job }),
  cancelAll: () => invoke<void>('cancel_all'),
  resolveHuman: (agent: string, ok: boolean) => invoke<boolean>('resolve_human', { agent, ok }),
  openLogin: (site: string) => invoke<void>('open_login', { site }),
  closeLogin: (done: boolean) => invoke<void>('close_login', { done }),
  visited: () => invoke<string[]>('visited'),
  markVisited: (url: string) => invoke<void>('mark_visited', { url }),
  openDataDir: () => invoke<void>('open_data_dir'),
  schedules: () => invoke<Schedule[]>('list_schedules'),
  saveSchedule: (schedule: Schedule) => invoke<Schedule>('save_schedule', { schedule }),
  deleteSchedule: (id: string) => invoke<void>('delete_schedule', { id }),
  runSchedule: (id: string) => invoke<void>('run_schedule', { id }),
  stopSchedule: (id: string) => invoke<void>('stop_schedule', { id }),
  scheduleResults: (id: string) => invoke<ScheduleResults>('schedule_results', { id }),
  onEvent: (cb: (ev: SearchEvent) => void) => listen<SearchEvent>('snipr-event', (e) => cb(e.payload)),
  onAuto: (cb: (ev: ScheduleEvent) => void) => listen<ScheduleEvent>('snipr-auto', (e) => cb(e.payload)),
  openUrl: (url: string) => openUrl(url),
};
