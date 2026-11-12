import { check, el, fold, icon } from './dom';
import { togglePop } from './popover';
import { siteById, state } from './state';
import { t } from './strings';

export interface StorePickOptions {
  selected: () => Set<string>;
  onChange: (next: Set<string>) => void;
  /** Number shown next to each store (e.g. results found). */
  counts?: () => Record<string, number | string>;
}

const DEFAULT_OFF = ['olx', 'enjoei'];

export function storesSummary(sel: Set<string>) {
  const ids = state.sites.filter((s) => sel.has(s.id)).map((s) => s.id);
  const total = state.sites.length;
  if (ids.length === total) return t.allStores;
  if (ids.length === 0) return t.noStore;
  if (ids.length === 1) return siteById(ids[0])?.name ?? '';
  return t.storesCount(ids.length, total);
}

/** Store multi-select. The list stays open while the user checks stores. */
export function storePicker(box: HTMLElement, o: StorePickOptions) {
  let q = '';
  let list: HTMLElement | null = null;
  let all: HTMLButtonElement | null = null;
  const btn = el('button', { type: 'button', 'aria-haspopup': 'listbox', 'aria-expanded': 'false' });

  const commit = (next: Set<string>) => {
    o.onChange(next);
    paint();
  };

  const paintList = () => {
    if (!list || !all) return;
    const sel = o.selected();
    const everything = state.sites.length > 0 && state.sites.every((s) => sel.has(s.id));
    all.textContent = everything ? t.deselectAll : t.selectAll;
    const counts = o.counts?.() ?? {};
    const f = fold(q.trim());
    const shown = state.sites.filter((s) => fold(s.name).includes(f) || fold(s.domain).includes(f));
    const focused = (document.activeElement as HTMLElement | null)?.dataset?.store;
    list.replaceChildren(
      ...(shown.length
        ? shown.map((s) =>
            check(
              sel.has(s.id),
              () => {
                const next = new Set(o.selected());
                if (next.has(s.id)) next.delete(s.id);
                else next.add(s.id);
                commit(next);
              },
              [
                el('span', { class: 'nm', title: s.name }, s.name, s.requires_login ? el('small', { text: t.requiresLogin }) : null),
                el('span', { class: 'n', text: counts[s.id] ?? '' }),
              ],
              { 'data-store': s.id },
            ),
          )
        : [el('div', { class: 'none', text: t.noStoreFound })]),
    );
    if (focused) list.querySelector<HTMLElement>(`[data-store="${CSS.escape(focused)}"]`)?.focus();
  };

  const build = (pop: HTMLElement) => {
    const find = el('input', {
      type: 'text',
      placeholder: t.findStore,
      'aria-label': t.findStore,
      value: q,
      spellcheck: 'false',
      oninput: (e: Event) => {
        q = (e.target as HTMLInputElement).value;
        paintList();
      },
    });
    list = el('div');
    all = el('button', {
      type: 'button',
      onclick: () => {
        const sel = o.selected();
        const everything = state.sites.every((s) => sel.has(s.id));
        commit(everything ? new Set() : new Set(state.sites.map((s) => s.id)));
      },
    });
    pop.append(
      el('label', { class: 'find' }, icon('i-search'), find),
      el(
        'div',
        { class: 'hd' },
        all,
        el('button', {
          type: 'button',
          text: t.restoreDefault,
          onclick: () => commit(new Set(state.sites.filter((s) => !s.requires_login && !DEFAULT_OFF.includes(s.id)).map((s) => s.id))),
        }),
      ),
      list,
    );
    paintList();
  };

  btn.addEventListener('click', (e) => {
    e.stopPropagation();
    togglePop(box, btn, build, true);
  });
  box.replaceChildren(btn);

  function paint() {
    btn.replaceChildren(el('span', { class: 'val', text: storesSummary(o.selected()) }), icon('i-down'));
    if (box.classList.contains('open')) paintList();
  }
  paint();
  return { paint };
}
