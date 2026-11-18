import { api } from './api';
import type { Listing } from './bindings/Listing';
import { el, errText, icon, toast } from './dom';
import { linkKey, state } from './state';
import { t } from './strings';

export const brl = (v: number) => v.toLocaleString('pt-BR', { style: 'currency', currency: 'BRL' });

function price(v: number, original?: number) {
  const c = Math.round((v % 1) * 100);
  const was =
    original && original > v
      ? el(
          'span',
          { class: 'was', role: 'tooltip' },
          el('s', { text: brl(original) }),
          el('span', { class: 'off', text: `-${Math.round((1 - v / original) * 100)}%` }),
        )
      : null;
  return el(
    'span',
    { class: 'price' + (was ? ' has-was' : ''), 'aria-label': was ? `${brl(v)}. ${t.wasPrice}: ${brl(original!)}` : brl(v) },
    el('span', { class: 'cur', text: 'R$' }),
    el('span', { class: 'int', text: Math.floor(v).toLocaleString('pt-BR') }),
    el('span', { class: 'ct', text: ',' + String(c).padStart(2, '0') }),
    was,
  );
}

function cardCell(l: Listing) {
  if (l.card_price == null && !l.installments) return el('span', { class: 'cc none', text: '—', title: t.noCardInfo });
  return el(
    'span',
    { class: 'cc' },
    l.card_price != null ? el('span', { class: 'v', text: brl(l.card_price) }) : null,
    l.installments ? el('span', { class: 'x' + (l.interest_free ? ' free' : ''), text: t.installments(l.installments, l.interest_free) }) : null,
  );
}

function openListing(l: Listing, row: HTMLElement) {
  if (!/^https?:\/\//i.test(l.url)) return;
  api.openUrl(l.url).catch((e) => toast(errText(e)));
  row.classList.add('seen');
  const key = linkKey(l.url);
  if (state.visited.has(key)) return;
  state.visited.add(key);
  api.markVisited(l.url).catch(() => {});
}

/** True when some listing has offers to show in the Information column. */
export const hasInfo = (all: Listing[]) => all.some((l) => l.offers.length > 0);

export function columns(info = true) {
  return el(
    'div',
    { class: 'cols' + (info ? '' : ' no-info'), 'aria-hidden': 'true' },
    el('span'),
    el('span', { text: t.colPix }),
    el('span', { text: t.colCard }),
    el('span', { text: t.colListing }),
    el('span', { text: t.colInfo }),
    el('span', { class: 'r', text: t.colStore }),
  );
}

export interface RowFlags {
  best?: boolean;
  fresh?: boolean;
  /** Not in the previous run of an automatic search. */
  recent?: boolean;
  /** The Information column is shown (some listing in the list has offers). */
  info?: boolean;
}

/** One result row. The whole row opens the listing. */
export function listingRow(l: Listing, f: RowFlags = {}) {
  const seen = state.visited.has(linkKey(l.url));
  const cls = ['row', f.best && 'best', f.fresh && 'fresh', seen && 'seen', f.recent && 'recent', f.info === false && 'no-info'].filter(Boolean).join(' ');
  const row: HTMLLIElement = el(
    'li',
    {
      class: cls,
      tabindex: '0',
      role: 'link',
      title: t.openListing(l.site_name),
      onclick: () => openListing(l, row),
      onkeydown: (e: KeyboardEvent) => {
        if (e.key === 'Enter' || e.key === ' ') {
          e.preventDefault();
          openListing(l, row);
        }
      },
    },
    el('span', { class: 'dot ' + (l.condition === 'used' ? 'u' : 'f'), title: l.condition === 'used' ? t.used : t.new }),
    price(l.price, l.original_price),
    cardCell(l),
    el(
      'div',
      { class: 't' },
      el(
        'div',
        { class: 'ti' },
        el('span', { class: 'tt', text: l.title }),
        f.recent ? el('span', { class: 'intl', text: t.recentBadge, title: t.recentTip }) : null,
        l.international ? el('span', { class: 'intl', text: t.intlBadge, title: t.intlTip }) : null,
      ),
      el('div', { class: 'sub' }, el('span', { class: 'site', text: l.site_name }), ...l.offers.map((o) => el('span', { class: 'of', text: o }))),
    ),
    el('div', { class: 'info' }, ...l.offers.map((o) => el('span', { class: 'chip', text: o, title: o }))),
    el('span', { class: 'dest' }, icon('i-out'), el('span', { text: l.site_name })),
  );
  return row;
}
