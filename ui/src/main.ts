import '@fontsource/hanken-grotesk/400.css';
import '@fontsource/hanken-grotesk/500.css';
import '@fontsource/hanken-grotesk/600.css';
import '@fontsource/hanken-grotesk/700.css';
import './styles.css';
import { finishLogin, renderAccounts } from './accounts';
import { $, $$, errText, toast, translate } from './dom';
import { initFooter } from './footer';
import { closePop, isPopOpen } from './popover';
import { initSearch, render } from './search';
import { initSettings, renderSettings } from './settings';
import { applyTheme, reloadSites, state } from './state';
import { initStores, openStoresModal } from './stores';
import { t } from './strings';

function openPanel(id: string) {
  closePop();
  $$('.sheet, .modal').forEach((x) => (x.hidden = x.id !== id));
  $('#side').classList.remove('open');
  $('#scrim').hidden = false;
}

function closePanels() {
  closePop();
  if (!$('#accounts').hidden) finishLogin(false);
  $$('.sheet, .modal').forEach((x) => (x.hidden = true));
  $('#side').classList.remove('open');
  $('#scrim').hidden = true;
  render();
}

function wire() {
  $('#o-settings').addEventListener('click', () => {
    renderSettings();
    openPanel('settings');
  });
  $('#o-accounts').addEventListener('click', () => {
    renderAccounts();
    openPanel('accounts');
  });
  $('#o-stores').addEventListener('click', (e) => {
    e.stopPropagation();
    openStoresModal();
    openPanel('stores-modal');
    setTimeout(() => $('#sm-q').focus(), 0);
  });
  $('#o-filters').addEventListener('click', () => {
    $('#side').classList.add('open');
    $('#scrim').hidden = false;
  });
  $('#scrim').addEventListener('click', closePanels);
  $$('[data-close]').forEach((b) => b.addEventListener('click', closePanels));
  document.addEventListener('keydown', (e) => {
    if (e.key === 'Escape') {
      if (isPopOpen()) closePop(true);
      else if (!$('#scrim').hidden) closePanels();
    }
    if (e.key === ',' && (e.metaKey || e.ctrlKey)) {
      e.preventDefault();
      renderSettings();
      openPanel('settings');
    }
  });
}

async function boot() {
  translate();
  $('#go').title = t.searchButton;
  wire();
  initSearch();
  initSettings();
  initFooter();
  initStores(render);
  try {
    await reloadSites();
    applyTheme(state.settings.theme);
    state.cond = new Set(state.settings.conditions.filter((c): c is 'new' | 'used' => c === 'new' || c === 'used'));
    if (!state.cond.size) state.cond.add('new');
    state.showIntl = state.settings.show_international;
    render();
    $('#q').focus();
  } catch (e) {
    toast(errText(e));
  }
}

boot();
