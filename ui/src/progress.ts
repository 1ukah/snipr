import type { SearchEvent } from './bindings/SearchEvent';
import { el, icon } from './dom';
import type { AgentView } from './state';
import { t } from './strings';

/** Progress panel: one line per store, with a bar and the last message. */
export function paintAgents(box: HTMLElement, list: AgentView[], show: boolean) {
  box.hidden = !show || !list.length;
  if (box.hidden) return;
  const done = list.filter((a) => a.st === 'ok' || a.st === 'fail').length;
  box.replaceChildren(
    el('div', { class: 'fl' }, icon('i-pulse'), el('span', { text: t.progress }), el('span', { class: 'n', text: `${done}/${list.length}` })),
    ...list.map((a) =>
      el(
        'div',
        { class: 'agent ' + a.st },
        el('span', { class: 'nm', text: a.label }),
        el('span', { class: 'trk' }, el('i', { style: `width:${a.pct}%` })),
        el('span', { class: 'm', text: a.msg, title: a.msg }),
        el('span', { class: 'st' }, a.st === 'ok' ? icon('i-check') : a.st === 'fail' ? icon('i-x') : null),
      ),
    ),
  );
}

/** Applies a store event (started, log, agent_finished) to the progress map. */
export function trackAgents(agents: Map<string, AgentView>, ev: SearchEvent) {
  if (ev.type === 'started') {
    agents.clear();
    for (const a of ev.agents) agents.set(a.id, { job: ev.job, label: a.label, sites: a.sites, st: 'wait', pct: 0, msg: t.queued });
  } else if (ev.type === 'log') {
    const a = agents.get(ev.agent);
    if (a) {
      a.st = 'run';
      a.msg = ev.message;
      a.pct = Math.min(92, a.pct + (92 - a.pct) * 0.12);
    }
  } else if (ev.type === 'agent_finished') {
    const a = agents.get(ev.agent);
    if (a) {
      a.st = ev.ok ? 'ok' : 'fail';
      a.pct = ev.ok ? 100 : a.pct;
      a.msg = ev.summary || (ev.ok ? t.doneState : t.failedState);
    }
  }
}
