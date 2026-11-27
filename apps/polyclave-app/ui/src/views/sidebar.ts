// Left navigation: Global Hub, Needs you and the collapsible project tree.
import { S, hubSessions, runLabel, runTone, totalNeedsCount } from '../state/store.js';
import type { Actions } from '../state/store.js';
import { withFocusPreserved } from '../state/focus.js';
import { basename, byId, h } from './elements.js';
import { icon } from './icons.js';

function saveCollapsed(): void {
  try { localStorage.setItem('pc-collapsed', JSON.stringify([...S.collapsed])); } catch { /* storage unavailable */ }
}
export function loadCollapsed(): void {
  try { for (const k of JSON.parse(localStorage.getItem('pc-collapsed') ?? '[]') as string[]) S.collapsed.add(k); } catch { /* ignore */ }
}

export function renderSidebar(act: Actions): void {
  const waitingTotal = totalNeedsCount();
  const count = byId('needs-count');
  count.hidden = waitingTotal === 0;
  count.textContent = String(waitingTotal);
  byId('needs-you').setAttribute('aria-label', 'Needs you' + (waitingTotal ? ', ' + waitingTotal + ' waiting' : ''));
  byId('global-hub').setAttribute('aria-pressed', String(S.global));
  const running = S.runs.filter(r => r.status === 'running').length;
  byId('global-meta').textContent = running ? running + ' running' : hubSessions().length ? 'All projects' : '';

  const tree = byId('projects');
  const projects = new Map<string, typeof S.sessions>();
  for (const s of hubSessions()) {
    const list = projects.get(s.repo_cwd) ?? [];
    list.push(s); projects.set(s.repo_cwd, list);
  }
  byId('project-count').textContent = projects.size ? String(projects.size) : '';

  withFocusPreserved(tree, () => {
    tree.replaceChildren();
    if (!projects.size) {
      tree.append(h('p', { class: 'tree-empty', text: S.online ? 'No projects yet. Use + to register a local repository.' : 'Projects appear once the local daemon is connected.' }));
      return;
    }
    for (const [repo, sessions] of projects) {
      const expanded = !S.collapsed.has(repo);
      const runs = S.runs.filter(r => sessions.some(s => s.id === r.hub_session_id));
      const live = runs.filter(r => r.status === 'running').length;
      const waiting = runs.filter(r => r.status === 'needs_review').length;
      const head = h('button', {
        type: 'button', class: 'project-head', 'aria-expanded': String(expanded), title: repo, 'data-focus-key': 'proj:' + repo,
        onclick: () => {
          if (S.collapsed.has(repo)) S.collapsed.delete(repo); else S.collapsed.add(repo);
          saveCollapsed(); renderSidebar(act);
        },
      }, icon('chevronDown', 'ic--sm chev'), h('span', { class: 'name', text: basename(repo) }),
        waiting ? h('span', { class: 'badge', text: String(waiting), title: waiting + ' waiting for review' }) :
          live ? h('span', { class: 'pill-tag', 'data-tone': 'run', text: live + ' running' }) : null);
      const body = h('div', { class: 'project-body' });
      for (const s of sessions) {
        const streaming = S.live.has(s.id);
        const b = h('button', {
          type: 'button', class: 'tree-item', 'aria-pressed': String(!S.global && s.id === S.selected), 'data-focus-key': 'sess:' + s.id,
          title: s.name, onclick: () => act.selectSession(s.id),
        },
        h('span', { class: 'dot ' + (streaming ? 'dot--think pulse' : 'dot--idle') }),
        h('span', { class: 't-title', text: s.name }),
        h('span', { class: 't-meta', text: streaming ? 'Thinking' : s.role === 'manager' ? 'Advisor' : 'Hub' }),
        h('span', { class: 't-sub', text: s.profile_id === 'builtin-hub' ? 'Local hub' : s.provider_model }));
        body.append(b);
        for (const r of runs.filter(x => x.hub_session_id === s.id).slice(-8).reverse()) {
          const tone = runTone(r.status);
          body.append(h('button', {
            type: 'button', class: 'tree-item is-run', 'aria-pressed': String(r.id === S.activeRun), 'data-focus-key': 'run:' + r.id,
            title: r.instructions.slice(0, 200), onclick: () => act.openRun(r.id, true),
          },
          h('span', { class: 'dot dot--' + (tone === 'run' ? 'run' : tone === 'review' ? 'review' : tone === 'done' ? 'done' : tone === 'fail' ? 'fail' : 'idle') }),
          h('span', { class: 't-title', text: r.instructions.split('\n')[0]?.slice(0, 80) || r.id }),
          h('span', { class: 't-meta', 'data-tone': tone, text: runLabel(r.status) }),
          h('span', { class: 't-sub', text: r.id + ' · ' + r.vendor + (r.model ? ' · ' + r.model : '') })));
        }
      }
      tree.append(h('section', { class: 'project' }, head, body));
    }
  });
}
