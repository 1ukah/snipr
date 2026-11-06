import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { openUrl } from '@tauri-apps/plugin-opener';
import type { CliStatus } from './bindings/CliStatus';
import type { Provider } from './bindings/Provider';
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
  checkSiteLogin: (id: string) => invoke<Site>('check_site_login', { id }),
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
  onEvent: (cb: (ev: SearchEvent) => void) => listen<SearchEvent>('snipr-event', (e) => cb(e.payload)),
  openUrl: (url: string) => openUrl(url),
};
