import { t, type StringKey } from './strings';

export const $ = <T extends Element = HTMLElement>(s: string, root: ParentNode = document) => root.querySelector(s) as T;
export const $$ = <T extends Element = HTMLElement>(s: string, root: ParentNode = document) => [...root.querySelectorAll(s)] as T[];

type Props = Record<string, unknown>;
type Child = Node | string | number | null | undefined | false | Child[];

export function el<K extends keyof HTMLElementTagNameMap>(tag: K, props: Props = {}, ...kids: Child[]): HTMLElementTagNameMap[K] {
  const e = document.createElement(tag);
  for (const [k, v] of Object.entries(props)) {
    if (v === undefined || v === null || v === false) continue;
    if (k === 'class') e.className = String(v);
    else if (k === 'text') e.textContent = String(v);
    else if (k.startsWith('on') && typeof v === 'function') e.addEventListener(k.slice(2), v as EventListener);
    else e.setAttribute(k, v === true ? '' : String(v));
  }
  append(e, kids);
  return e;
}

function append(e: Element, kids: Child[]) {
  for (const k of kids) {
    if (k === null || k === undefined || k === false) continue;
    if (Array.isArray(k)) append(e, k);
    else e.append(k instanceof Node ? k : document.createTextNode(String(k)));
  }
}

const NS = 'http://www.w3.org/2000/svg';
export function icon(id: string): SVGSVGElement {
  const s = document.createElementNS(NS, 'svg');
  s.setAttribute('class', 'i');
  const u = document.createElementNS(NS, 'use');
  u.setAttribute('href', '#' + id);
  s.append(u);
  return s;
}

export function translate(root: ParentNode = document) {
  const str = (k: string) => String(t[k as StringKey] ?? k);
  $$('[data-t]', root).forEach((n) => (n.textContent = str(n.dataset.t!)));
  $$('[data-t-placeholder]', root).forEach((n) => n.setAttribute('placeholder', str(n.dataset.tPlaceholder!)));
  $$('[data-t-title]', root).forEach((n) => n.setAttribute('title', str(n.dataset.tTitle!)));
  $$('[data-t-aria]', root).forEach((n) => n.setAttribute('aria-label', str(n.dataset.tAria!)));
}

export function check(on: boolean, onclick: () => void, kids: Child[], props: Props = {}) {
  return el('button', { type: 'button', class: 'ck', role: 'checkbox', 'aria-checked': String(on), onclick, ...props }, el('span', { class: 'box' }, icon('i-check')), ...kids);
}

export const fold = (s: string) => s.normalize('NFD').replace(/[̀-ͯ]/g, '').toLowerCase();

let toastTimer = 0;
export function toast(msg: string) {
  const box = $('#toast');
  box.textContent = msg;
  box.hidden = false;
  clearTimeout(toastTimer);
  toastTimer = window.setTimeout(() => (box.hidden = true), Math.min(9000, 2400 + msg.length * 40));
}

export const errText = (e: unknown) => (e instanceof Error ? e.message : String(e));
