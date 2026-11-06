import { api } from './api';
import type { AuthMode } from './bindings/AuthMode';
import type { CliStatus } from './bindings/CliStatus';
import type { Provider } from './bindings/Provider';
import type { Settings } from './bindings/Settings';
import { $, el, errText, fold, icon, toast } from './dom';
import { closePop, segment, select, togglePop } from './popover';
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

function renderLogin() {
  const box = $('#auth-login');
  box.replaceChildren(
    ...LOGIN_PROVIDERS.map((p) => {
      const s = statuses.find((x) => x.provider === p);
      const on = ai().login_provider === p;
      let right: HTMLElement;
      if (!s) right = el('span', { class: 'ok' });
      else if (!s.installed)
        right = el(
          'span',
          { class: 'btn', role: 'button', tabindex: '0', onclick: (e: Event) => (e.stopPropagation(), api.openUrl(installUrl[p])) },
          icon('i-out'),
          t.install,
        );
      else if (s.logged_in === true) right = el('span', { class: 'ok' }, icon('i-check'), t.connected);
      else
        right = el(
          'span',
          {
            class: 'btn',
            role: 'button',
            tabindex: '0',
            onclick: async (e: Event) => {
              e.stopPropagation();
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
      const sub = s && s.installed && s.logged_in === null ? `${t.providerSub[p]} · ${t.installed}` : s && !s.installed ? `${t.providerSub[p]} · ${t.notInstalled}` : t.providerSub[p];
      return el(
        'button',
        {
          type: 'button',
          class: 'prow' + (on ? ' on' : ''),
          onclick: () => {
            saveAi({ login_provider: p, model: '' }, true);
            renderAi();
          },
        },
        el('span', { class: 'rd' }),
        el('span', { class: 'nm' }, LOGIN_NAME[p], el('small', { text: sub })),
        right,
      );
    }),
  );
}

async function refreshStatuses() {
  statuses = await api.aiStatus().catch(() => []);
  if (!$('#settings').hidden) renderLogin();
}

function modelSource(): { key: string; provider: Provider; endpoint: string } | null {
  if (ai().mode !== 'api_key') return null;
  return { key: `${ai().key_provider}|${ai().endpoint}`, provider: ai().key_provider, endpoint: ai().endpoint };
}

function renderModel() {
  const box = $('#model');
  const input = el('input', { value: ai().model, placeholder: t.automatic, spellcheck: 'false', 'aria-label': t.model });
  const src = modelSource();
  const shell = el('div', { class: 'combo-in' }, el('span', { class: 'pre', text: t.model }), input);
  const paintList = (pop: HTMLElement) => {
    const s = modelSource();
    if (!s) return;
    const list = models.get(s.key);
    const q = fold(input.value.trim());
    const rows: HTMLElement[] = [];
    if (!list) rows.push(el('div', { class: 'none', text: t.loadingModels }));
    else {
      const shown = list.filter((m) => fold(m).includes(q)).slice(0, 200);
      if (!shown.length) rows.push(el('div', { class: 'none', text: t.noModels }));
      shown.forEach((m) =>
        rows.push(
          el(
            'button',
            {
              type: 'button',
              class: 'opt',
              role: 'option',
              'aria-selected': String(m === ai().model),
              onclick: () => {
                input.value = m;
                saveAi({ model: m }, true);
                closePop();
              },
            },
            el('span', { class: 'tick' }, icon('i-check')),
            m,
          ),
        ),
      );
    }
    pop.replaceChildren(...rows);
  };
  const open = () => {
    const s = modelSource();
    if (!s || box.classList.contains('open')) return;
    togglePop(box, input, (pop) => {
      paintList(pop);
      if (!models.has(s.key)) {
        api
          .listModels(s.provider, s.endpoint)
          .then((list) => models.set(s.key, list))
          .catch((e) => {
            models.set(s.key, []);
            toast(errText(e));
          })
          .finally(() => {
            const pop2 = box.querySelector<HTMLElement>('.pop');
            if (pop2) paintList(pop2);
          });
      }
    });
    input.focus();
  };
  input.addEventListener('focus', open);
  input.addEventListener('click', (e) => e.stopPropagation());
  input.addEventListener('input', () => {
    const pop = box.querySelector<HTMLElement>('.pop');
    if (pop) paintList(pop);
  });
  input.addEventListener('change', () => saveAi({ model: input.value.trim() }));
  if (src) {
    shell.append(
      el(
        'button',
        {
          type: 'button',
          class: 'ib sm',
          title: t.model,
          'aria-label': t.model,
          onclick: (e: Event) => {
            e.stopPropagation();
            open();
          },
        },
        icon('i-down'),
      ),
    );
  }
  box.replaceChildren(shell);
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
        saveAi({ key_provider: v, model: '' }, true);
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
  renderModel();
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
      saveAi({ mode: v, model: '' }, true);
      renderAi();
    },
  );
  $<HTMLInputElement>('#endpoint').onchange = (e) => {
    saveAi({ endpoint: (e.target as HTMLInputElement).value.trim() });
    models.clear();
  };
  select($('#effort'), {
    pre: t.reasoning,
    value: s.ai.effort,
    options: ['', 'low', 'medium', 'high', 'max'].map((v) => [v, t.effort[v]] as [string, string]),
    onChange: (v) => saveAi({ effort: v }),
  });
  renderAi();

  const why = (m: string) => ($('#why-a').textContent = m === 'single' ? t.whySingle : t.whyMulti);
  segment(
    $('#amode'),
    [
      ['multi', t.perStore, 'i-many'],
      ['single', t.single, 'i-one'],
    ],
    s.agent_mode,
    (v) => {
      why(v);
      saveSettings({ agent_mode: v });
    },
  );
  why(s.agent_mode);
  numberInput('#max-parallel', s.max_parallel, (n) => saveSettings({ max_parallel: Math.min(8, Math.max(1, n)) }));
  numberInput('#max-steps', s.max_steps, (n) => saveSettings({ max_steps: Math.min(200, Math.max(5, n)) }));

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
  chrome.onchange = () => saveSettings({ chrome_path: chrome.value.trim() });
  const cli = $<HTMLInputElement>('#cli-path');
  cli.value = s.ai.cli_path;
  cli.onchange = () => {
    saveAi({ cli_path: cli.value.trim() }, true);
    refreshStatuses();
  };
  refreshStatuses();
}

export function initSettings() {
  window.addEventListener('focus', () => {
    if (!$('#settings').hidden && ai().mode === 'login') refreshStatuses();
  });
}
