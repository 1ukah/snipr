import { api } from './api';
import type { AuthMode } from './bindings/AuthMode';
import type { CliStatus } from './bindings/CliStatus';
import type { Provider } from './bindings/Provider';
import type { Settings } from './bindings/Settings';
import { $, el, errText, icon, toast } from './dom';
import { renderFooter } from './footer';
import { segment, select } from './popover';
import { applyTheme, saveSettings, state } from './state';
import { installUrl, t } from './strings';

const KEY_PROVIDERS: Provider[] = ['anthropic', 'openai', 'google', 'xai', 'openrouter', 'ollama', 'custom'];
const LOGIN_PROVIDERS: Provider[] = ['anthropic', 'openai', 'google'];
const LOGIN_NAME: Record<string, string> = { anthropic: 'Anthropic', openai: 'OpenAI', google: 'Google' };

let statuses: CliStatus[] = [];
const models = new Map<string, string[]>();

const ai = () => state.settings.ai;
const saveAi = (patch: Partial<Settings['ai']>, now = false) => saveSettings({ ai: { ...ai(), ...patch } }, now);

function secretField(box: HTMLElement, name: string, placeholder: string) {
  const input = el('input', { type: 'password', placeholder, spellcheck: 'false', autocomplete: 'off', 'aria-label': placeholder });
  const status = el('span', { class: 'st-set' });
  const store = async (value: string) => {
    input.value = '';
    value = value.trim();
    if (!value) return;
    try {
      await api.setSecret(name, value);
      toast(t.savedToast);
      models.clear();
      renderModels();
    } catch (e) {
      toast(errText(e));
    }
    paint();
  };
  const paint = async () => {
    const has = (await api.secretStatus([name]).catch(() => ({}) as Record<string, boolean>))[name];
    status.replaceChildren(
      ...(has
        ? [
            el('span', { text: t.saved }),
            el(
              'button',
              {
                type: 'button',
                class: 'ib sm',
                title: t.remove,
                'aria-label': t.remove,
                onclick: async () => {
                  await api.deleteSecret(name).catch((e) => toast(errText(e)));
                  models.clear();
                  renderModels();
                  paint();
                },
              },
              icon('i-trash'),
            ),
          ]
        : []),
    );
  };
  input.addEventListener('paste', (e) => {
    e.preventDefault();
    store(e.clipboardData?.getData('text') ?? '');
  });
  input.addEventListener('keydown', (e) => {
    if (e.key === 'Enter') {
      e.preventDefault();
      store(input.value);
    }
  });
  input.addEventListener('blur', () => input.value && store(input.value));
  box.replaceChildren(icon('i-lock'), input, status);
  paint();
}

function loginState(p: Provider): HTMLElement {
  const s = statuses.find((x) => x.provider === p);
  if (!s) return el('span', { class: 'lstate' });
  if (!s.installed)
    return el('button', { type: 'button', class: 'btn', onclick: () => api.openUrl(installUrl[p]) }, icon('i-out'), t.install);
  if (s.logged_in === true) return el('span', { class: 'lstate' }, icon('i-check'), t.connected);
  return el(
    'button',
    {
      type: 'button',
      class: 'btn',
      onclick: async () => {
        try {
          await api.openAiLogin(p);
          toast(t.loginTerminal);
        } catch (err) {
          toast(errText(err));
        }
      },
    },
    icon('i-arrow'),
    t.enter,
  );
}

function renderLogin() {
  const box = $('#auth-login');
  const pick = el('div', { class: 'sel' });
  const p = ai().login_provider;
  select(pick, {
    pre: t.provider,
    value: p,
    options: LOGIN_PROVIDERS.map((x) => [x, `${LOGIN_NAME[x]} (${t.providerSub[x]})`] as [Provider, string]),
    onChange: (v) => {
      saveAi({ login_provider: v, model: '', fast_model: '' }, true);
      renderAi();
    },
  });
  box.replaceChildren(el('div', { class: 'pick' }, pick, loginState(p)));
}

async function refreshStatuses() {
  statuses = await api.aiStatus().catch(() => []);
  if (!$('#settings').hidden) renderLogin();
}

function modelSource(): { key: string; load: () => Promise<string[]> } {
  const a = ai();
  if (a.mode === 'api_key') return { key: `${a.key_provider}|${a.endpoint}`, load: () => api.listModels(a.key_provider, a.endpoint) };
  const p = a.mode === 'oauth' ? 'anthropic' : a.login_provider;
  return { key: `cli|${p}`, load: () => api.cliModels(p) };
}

type ModelKey = 'model' | 'fast_model';

function renderModels() {
  const src = modelSource();
  const paint = () => {
    renderModel('#model', 'model', t.model, models.get(src.key) ?? []);
    renderModel('#fast-model', 'fast_model', t.fastModel, models.get(src.key) ?? []);
  };
  paint();
  if (models.has(src.key)) return;
  src
    .load()
    .catch(() => [] as string[])
    .then((list) => {
      models.set(src.key, list);
      if (modelSource().key === src.key) paint();
    });
}

function renderModel(sel: string, key: ModelKey, label: string, list: string[]) {
  const cur = ai()[key];
  const options: [string, string][] = [['', t.automatic], ...list.map((m) => [m, m] as [string, string])];
  if (cur && !list.includes(cur)) options.push([cur, cur]);
  select($(sel), { pre: label, value: cur, options, onChange: (v) => saveAi({ [key]: v }, true) });
}

function effortLevels(): string[] {
  const a = ai();
  if (a.mode !== 'api_key' || a.key_provider === 'anthropic') return ['low', 'medium', 'high', 'xhigh', 'max'];
  if (a.key_provider === 'openai') return ['low', 'medium', 'high', 'xhigh'];
  return ['low', 'medium', 'high'];
}

function renderEffort() {
  const levels = effortLevels();
  if (ai().effort && !levels.includes(ai().effort)) saveAi({ effort: '' }, true);
  select($('#effort'), {
    pre: t.reasoning,
    value: ai().effort,
    options: ['', ...levels].map((v) => [v, t.effort[v]] as [string, string]),
    onChange: (v) => saveAi({ effort: v }),
  });
}

function renderAi() {
  const mode = ai().mode;
  $('#auth-login').hidden = mode !== 'login';
  $('#auth-key').hidden = mode !== 'api_key';
  $('#auth-oauth').hidden = mode !== 'oauth';
  $('#auth-note').hidden = mode === 'login';
  if (mode === 'login') renderLogin();
  if (mode === 'api_key') {
    const p = ai().key_provider;
    select($('#key-prov'), {
      pre: t.provider,
      value: p,
      options: KEY_PROVIDERS.map((x) => [x, t.providerName[x]] as [Provider, string]),
      onChange: (v) => {
        saveAi({ key_provider: v, model: '', fast_model: '' }, true);
        renderAi();
      },
    });
    $('#key-endpoint').hidden = p !== 'custom';
    $<HTMLInputElement>('#endpoint').value = ai().endpoint;
    const keyBox = $('#key-secret');
    if (p === 'ollama') {
      keyBox.hidden = true;
      $('#auth-note').hidden = true;
    } else {
      keyBox.hidden = false;
      secretField(keyBox, `api_key:${p}`, t.pasteKey);
    }
  }
  if (mode === 'oauth') secretField($('#oauth-secret'), 'oauth:anthropic', t.pasteToken);
  renderModels();
  renderEffort();
}

async function renderBrowser() {
  const status = await api.browserStatus().catch(() => null);
  const warn = $('#browser-warn');
  const box = $('#browser-pick');
  if (!status) return;
  const found = status.browsers;
  const custom = state.settings.chrome_path.trim();
  const label = (b: (typeof found)[number]) => (b.version ? `${b.name} ${b.version.split('.')[0]}` : b.name);
  const options: [string, string][] = [['', found.length ? t.autoBrowser(label(found[0])) : t.automatic], ...found.map((b) => [b.path, label(b)] as [string, string])];
  if (custom && !found.some((b) => b.path === custom)) options.push([custom, custom]);
  select(box, {
    pre: t.browserApp,
    value: custom,
    options,
    onChange: (v) => {
      saveSettings({ chrome_path: v }, true);
      $<HTMLInputElement>('#chrome-path').value = v;
      renderBrowser();
    },
  });
  const selected = found.find((b) => b.path === status.selected);
  const msg = !found.length && !status.selected ? t.noBrowser : status.custom_missing ? t.customMissing : selected?.name === 'Brave' ? t.braveNote : '';
  warn.textContent = msg;
  warn.hidden = !msg;
  ($('#browser-test') as HTMLButtonElement).disabled = !status.selected;
}

async function testBrowser() {
  const btn = $<HTMLButtonElement>('#browser-test');
  btn.disabled = true;
  btn.classList.add('busy');
  try {
    toast(t.browserOk(await api.testBrowser()));
  } catch (e) {
    toast(errText(e));
  }
  btn.disabled = false;
  btn.classList.remove('busy');
}

function numberInput(sel: string, value: number, apply: (n: number) => void) {
  const input = $<HTMLInputElement>(sel);
  input.value = String(value);
  input.onchange = () => {
    const n = parseInt(input.value, 10);
    if (Number.isFinite(n) && n >= 0) apply(n);
    else input.value = String(value);
  };
}

function toggle(sel: string, value: boolean, apply: (v: boolean) => void) {
  const b = $(sel);
  b.setAttribute('aria-checked', String(value));
  b.onclick = () => {
    const v = b.getAttribute('aria-checked') !== 'true';
    b.setAttribute('aria-checked', String(v));
    apply(v);
  };
}

export function renderSettings() {
  const s = state.settings;
  segment(
    $('#theme'),
    [
      ['system', t.themeSystem, 'i-system'],
      ['dark', t.themeDark, 'i-moon'],
      ['light', t.themeLight, 'i-sun'],
    ],
    s.theme,
    (v) => {
      applyTheme(v);
      saveSettings({ theme: v }, true);
    },
  );
  segment<AuthMode>(
    $('#auth'),
    [
      ['login', t.login, 'i-user'],
      ['api_key', t.apiKey, 'i-key'],
      ['oauth', t.oauth, 'i-lock'],
    ],
    s.ai.mode,
    (v) => {
      saveAi({ mode: v, model: '', fast_model: '' }, true);
      renderAi();
    },
  );
  $<HTMLInputElement>('#endpoint').onchange = (e) => {
    saveAi({ endpoint: (e.target as HTMLInputElement).value.trim() });
    models.clear();
    renderModels();
  };
  renderAi();
  toggle('#t-footer', s.status_bar, (v) => {
    saveSettings({ status_bar: v });
    renderFooter();
  });

  toggle('#t-offers', s.offer_details, (v) => saveSettings({ offer_details: v }));
  toggle('#t-hist-manual', s.history_manual, (v) => saveSettings({ history_manual: v }));
  toggle('#t-hist-auto', s.history_auto, (v) => saveSettings({ history_auto: v }));

  numberInput('#max-parallel', s.max_parallel, (n) => saveSettings({ max_parallel: Math.min(8, Math.max(1, n)) }));
  numberInput('#max-steps', s.max_steps, (n) => saveSettings({ max_steps: Math.min(60, Math.max(5, n)) }));
  renderBrowser();

  const whyB: Record<string, string> = { hidden: t.whyHidden, headless: t.whyHeadless, visible: t.whyVisible };
  segment(
    $('#bmode'),
    [
      ['hidden', t.hidden, 'i-hidden'],
      ['headless', t.headless, 'i-headless'],
      ['visible', t.visible, 'i-eye'],
    ],
    s.browser_mode,
    (v) => {
      $('#why-b').textContent = whyB[v];
      saveSettings({ browser_mode: v });
    },
  );
  $('#why-b').textContent = whyB[s.browser_mode];
  toggle('#t-media', s.block_media, (v) => saveSettings({ block_media: v }));
  toggle('#t-memory', s.low_memory, (v) => saveSettings({ low_memory: v }));
  numberInput('#idle', s.idle_close_secs, (n) => saveSettings({ idle_close_secs: n }));
  const chrome = $<HTMLInputElement>('#chrome-path');
  chrome.value = s.chrome_path;
  chrome.onchange = () => {
    saveSettings({ chrome_path: chrome.value.trim() }, true);
    renderBrowser();
  };
  const cli = $<HTMLInputElement>('#cli-path');
  cli.value = s.ai.cli_path;
  cli.onchange = () => {
    saveAi({ cli_path: cli.value.trim() }, true);
    refreshStatuses();
    models.clear();
    renderModels();
  };
  refreshStatuses();
}

export function initSettings() {
  $('#browser-test').addEventListener('click', testBrowser);
  $('#open-data').addEventListener('click', () => api.openDataDir().catch((e) => toast(errText(e))));
  window.addEventListener('focus', () => {
    if (!$('#settings').hidden && ai().mode === 'login') refreshStatuses();
  });
}
