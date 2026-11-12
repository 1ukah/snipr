import { el, icon } from './dom';

interface Open {
  box: HTMLElement;
  btn: HTMLElement;
  pop: HTMLElement;
}

let open: Open | null = null;

export function closePop(focusButton = false) {
  if (!open) return;
  const { box, btn, pop } = open;
  open = null;
  box.classList.remove('open');
  pop.remove();
  btn.setAttribute('aria-expanded', 'false');
  if (focusButton) btn.focus();
}

export const isPopOpen = () => open !== null;
export const popOwner = () => open?.box ?? null;

export function togglePop(box: HTMLElement, btn: HTMLElement, build: (pop: HTMLElement) => void, wide = false) {
  if (open && open.box === box) return closePop();
  closePop();
  const pop = el('div', { class: 'pop' + (wide ? ' wide' : ''), role: 'listbox' });
  build(pop);
  box.append(pop);
  box.classList.add('open');
  btn.setAttribute('aria-expanded', 'true');
  open = { box, btn, pop };
  const target = pop.querySelector<HTMLElement>('input, [aria-selected="true"], button');
  target?.focus();
}

// composedPath: a click that re-renders the list removes its own target from the DOM.
document.addEventListener('click', (e) => {
  if (open && !e.composedPath().includes(open.box)) closePop();
});

export interface SelectOptions<V extends string> {
  pre?: string;
  options: [V, string][];
  value: V;
  onChange: (v: V) => void;
}

export function select<V extends string>(box: HTMLElement, o: SelectOptions<V>) {
  let cur = o.value;
  const btn = el('button', { type: 'button', 'aria-haspopup': 'listbox', 'aria-expanded': 'false' });
  const paint = () =>
    btn.replaceChildren(
      ...(o.pre ? [el('span', { class: 'pre', text: o.pre })] : []),
      el('span', { class: 'val', text: o.options.find(([v]) => v === cur)?.[1] ?? '' }),
      icon('i-down'),
    );
  paint();
  btn.addEventListener('click', (e) => {
    e.stopPropagation();
    togglePop(box, btn, (pop) =>
      pop.append(
        ...o.options.map(([v, label]) =>
          el(
            'button',
            {
              type: 'button',
              class: 'opt',
              role: 'option',
              'aria-selected': String(v === cur),
              onclick: () => {
                cur = v;
                paint();
                closePop(true);
                o.onChange(v);
              },
            },
            el('span', { class: 'tick' }, icon('i-check')),
            label,
          ),
        ),
      ),
    );
  });
  box.replaceChildren(btn);
  return { set: (v: V) => ((cur = v), paint()) };
}

export function segment<V extends string>(box: HTMLElement, options: [V, string, string?][], value: V, onChange: (v: V) => void) {
  const paint = (cur: V) =>
    box.replaceChildren(
      ...options.map(([v, label, ic]) =>
        el(
          'button',
          {
            type: 'button',
            class: v === cur ? 'on' : '',
            'aria-pressed': String(v === cur),
            onclick: () => {
              paint(v);
              onChange(v);
            },
          },
          ic ? icon(ic) : null,
          label,
        ),
      ),
    );
  paint(value);
  return { set: paint };
}
