import { api } from './api';
import { $, el, errText, icon, toast } from './dom';
import { reloadSites, state } from './state';
import { t } from './strings';

let pending: string | null = null;

export function renderAccounts() {
  const sites = [...state.sites].sort((a, b) => Number(b.requires_login) - Number(a.requires_login));
  $('#site-rows').replaceChildren(
    ...sites.map((s) => {
      const st = state.settings.sessions[s.id];
      return el(
        'div',
        { class: 'srow' },
        el('span', { class: 'nm' }, s.name, s.requires_login ? el('small', { text: t.requiresLogin }) : null),
        el('span', { class: 'state' + (st === 'ok' ? ' on' : ''), text: st === 'ok' ? t.connected : st === 'expired' ? t.expired : '' }),
        el('button', { class: 'btn', onclick: () => openLogin(s.id) }, icon('i-arrow'), st ? t.enterAgain : t.enter),
      );
    }),
  );
  const bar = $('#login-bar');
  bar.hidden = !pending;
  if (pending) {
    bar.replaceChildren(el('p', { text: t.loginWindow }), el('button', { class: 'btn pri', onclick: () => finishLogin(true) }, icon('i-check'), t.done));
  }
}

async function openLogin(id: string) {
  try {
    await api.openLogin(id);
    pending = id;
  } catch (e) {
    toast(errText(e));
  }
  renderAccounts();
}

export async function finishLogin(done: boolean) {
  if (!pending) return;
  pending = null;
  await api.closeLogin(done).catch(() => {});
  await reloadSites();
  renderAccounts();
}
