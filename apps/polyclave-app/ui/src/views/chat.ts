// Center pane: breadcrumb, message stream with thinking cards, tool calls and Worker run cards, live turn.
import type { Api, HubMessage } from '../api/client.js';
import type { JsonValue, RunDto } from '../generated/proto.js';
import { S, hubSessions, runLabel, runTone, selectedSession, sessionRuns, vendorOf } from '../state/store.js';
import type { Actions } from '../state/store.js';
import { withFocusPreserved } from '../state/focus.js';
import { basename, button, byId, compact, h, richText, short } from './elements.js';
import { icon } from './icons.js';
import { createTicket, rememberRule } from './flows.js';
import { currentModelLabel } from './model-select.js';

const rendered = new Set<string>();

interface ToolCall { name: string; args: string }
function toolCalls(m: HubMessage): ToolCall[] {
  const meta = (m.metadata ?? m.metadata_json) as JsonValue | undefined;
  if (!meta || typeof meta !== 'object' || Array.isArray(meta)) return [];
  const calls = meta.tool_calls;
  if (!Array.isArray(calls)) return [];
  return calls.flatMap(c => {
    if (!c || typeof c !== 'object' || Array.isArray(c)) return [];
    const fn = c.function;
    if (fn && typeof fn === 'object' && !Array.isArray(fn)) return [{ name: String(fn.name ?? 'tool'), args: String(fn.arguments ?? '') }];
    return [{ name: String(c.name ?? 'tool'), args: '' }];
  });
}

function thinkCard(text: string, open = false, live = false): HTMLDetailsElement {
  const d = h('details', { class: 'think-card' },
    h('summary', null, h('span', { class: 'dot dot--think' + (live ? ' pulse' : '') }), live ? 'Thinking' : 'Thought process', icon('chevronDown', 'ic--sm')),
    h('div', { class: 'think-body', text }));
  d.open = open;
  return d;
}

function messageNode(api: Api, act: Actions, m: HubMessage): HTMLElement {
  const key = 'm' + m.id;
  const enter = !rendered.has(key);
  rendered.add(key);
  const cls = 'msg msg--' + m.source + (enter ? ' enter' : '');
  if (m.source === 'user') {
    const tools = h('div', { class: 'msg-tools' },
      button([icon('memory', 'ic--sm'), 'Remember this rule'], () => rememberRule(api, act, m.text, { message_id: m.id }), undefined, { 'data-focus-key': 'remember-' + m.id }),
      button([icon('ticket', 'ic--sm'), 'Create ticket'], () => createTicket(api, act, m.id, m.text), undefined, { 'data-focus-key': 'ticket-' + m.id }));
    return h('article', { class: cls, 'data-kind': 'user' },
      h('header', { class: 'msg-head' }, m.queued ? h('span', { class: 'queued', text: 'Queued' }) : null, h('span', { class: 'msg-who', text: 'You' })),
      h('div', { class: 'msg-bubble msg-body' }, richText(m.text)), tools);
  }
  if (m.source === 'host') {
    return h('article', { class: cls, 'data-kind': 'host' }, h('p', { class: 'msg-body', text: m.text }));
  }
  const card = h('div', { class: 'msg-card' });
  if (m.reasoning) card.append(thinkCard(m.reasoning));
  const calls = toolCalls(m);
  if (calls.length) {
    card.append(h('div', { class: 'tool-list', 'aria-label': 'Tool calls' }, ...calls.map(c =>
      h('div', { class: 'tool-row' }, icon('tool', 'ic--sm'), h('span', { class: 'name', text: c.name }), h('span', { class: 'args', text: c.args }), h('span', { class: 'pill-tag', 'data-tone': 'done', text: 'done' })))));
  }
  if (m.text) card.append(h('div', { class: 'msg-body' }, richText(m.text)));
  return h('article', { class: cls, 'data-kind': 'agent' },
    h('header', { class: 'msg-head' }, h('span', { class: 'msg-who', text: 'Hub' }), m.provider_label ? h('span', { text: m.provider_label }) : null),
    card);
}

export function runCard(act: Actions, r: RunDto, opts: { compact?: boolean } = {}): HTMLElement {
  const tone = runTone(r.status);
  const live = S.live.get(r.id);
  const pct = r.status === 'done' ? 100 : Math.round(100 * Math.min(1, r.current_attempt / Math.max(1, r.max_attempts)));
  const sha = r.observed_sha ?? r.result_sha;
  const card = h('article', { class: 'card run-card' + (opts.compact ? ' compact' : ''), 'data-tone': tone },
    h('div', { class: 'run-top' },
      h('span', { class: 'run-id', text: r.id }),
      h('span', { class: 'vendor-tag', 'data-vendor': r.vendor, text: r.vendor === 'codex' ? 'Codex' : 'Claude' }),
      r.model ? h('span', { class: 'vendor-tag', text: r.model }) : null,
      h('span', { class: 'grow' }),
      h('span', { class: 'pill-tag', 'data-tone': tone, text: runLabel(r.status) + (r.status === 'needs_review' && sha ? ' · ' + short(sha) : '') })),
    h('p', { class: 'run-text', text: r.instructions }),
    live?.text ? h('p', { class: 'run-meta', text: '› ' + live.text.slice(-140) }) : null,
    r.status === 'failed' && r.detail ? h('p', { class: 'card-sub', text: r.detail }) : null,
    h('div', { class: 'run-meta' },
      h('span', { text: 'Attempt ' + r.current_attempt + '/' + r.max_attempts }),
      h('span', { class: 'progress grow', 'data-tone': tone, style: '--value:' + pct + '%' }, h('i')),
      h('span', { text: r.base_branch + ' @ ' + short(r.base_sha) })),
    h('div', { class: 'run-actions' },
      button([icon('review', 'ic--sm'), r.status === 'needs_review' ? 'Review ' + short(sha) : 'Open'], () => act.openRun(r.id, true), undefined, { 'data-focus-key': 'open-' + r.id })));
  return card;
}

function overview(act: Actions): HTMLElement {
  const wrap = h('section', { class: 'welcome' },
    h('h2', { text: 'Global Hub' }),
    h('p', { text: 'Every registered project at a glance. Choose a project Hub to talk to it, or open a Worker run to review its evidence.' }));
  const grid = h('div', { class: 'overview' });
  for (const s of hubSessions()) {
    const runs = sessionRuns(s.id);
    grid.append(h('button', { type: 'button', class: 'card', onclick: () => act.selectSession(s.id), 'data-focus-key': 'ov-' + s.id },
      h('span', { class: 'card-title', text: s.name }),
      h('span', { class: 'card-sub mono', text: basename(s.repo_cwd) }),
      h('span', { class: 'stats' },
        h('span', { text: runs.filter(r => r.status === 'running').length + ' running' }),
        h('span', { text: runs.filter(r => r.status === 'needs_review').length + ' to review' }),
        h('span', { text: runs.filter(r => r.status === 'done').length + ' verified' }))));
  }
  if (!grid.childElementCount) grid.append(h('p', { class: 'empty', text: 'No projects registered yet.' }));
  wrap.append(grid);
  return wrap;
}

function welcome(): HTMLElement {
  return h('section', { class: 'welcome' },
    h('h2', { text: S.selected ? 'Start the conversation' : 'A local place to coordinate your agents' }),
    h('p', { text: 'The Hub plans with you, records tickets and dispatches Workers into isolated attempts. Nothing leaves this machine except the provider calls you choose.' }),
    h('ol', { class: 'steps' },
      h('li', { text: 'Register a local repository with +.' }),
      h('li', { text: 'Pick a provider and model in the selector above.' }),
      h('li', { text: 'Describe the work; turn a message into a ticket when it is ready for a Worker.' })));
}

export function renderChat(api: Api, act: Actions): void {
  const s = selectedSession();
  byId('crumb-project').textContent = S.global ? 'All projects' : s ? basename(s.repo_cwd) : 'Polyclave';
  byId('title').textContent = S.global ? 'Global Hub' : s?.name ?? 'Workspace Hub';

  const ticket = s ? S.tickets.filter(t => t.hub === s.id && t.status !== 'done' && t.status !== 'cancelled').slice(-1)[0] : undefined;
  const chip = byId<HTMLButtonElement>('ticket-chip');
  chip.hidden = !ticket || S.global;
  if (ticket) {
    chip.textContent = 'Ticket ' + ticket.id + ' · ' + ticket.used + '/' + ticket.limit + ' runs';
    chip.dataset.tone = ticket.status === 'blocked_on_user' ? 'review' : '';
    chip.onclick = () => act.setPanel('Topology');
  }

  const scroller = byId('history-scroll');
  const atBottom = scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight < 64;
  const queued = S.global ? [] : S.messages.filter(m => m.queued && m.source === 'user');
  const visibleMessages = S.messages.filter(m => !(m.queued && m.source === 'user'));
  const tray = byId<HTMLDetailsElement>('queued-tray');
  tray.hidden = queued.length === 0;
  if (queued.length) {
    byId('queued-count').textContent = String(queued.length);
    byId('queued-list').replaceChildren(...queued.map((m, idx) =>
      h('li', { class: 'queued-item' },
        h('span', { class: 'queued-text', text: m.text }),
        h('span', { class: 'queued-pill', text: '#' + (idx + 1) }))));
  } else {
    byId('queued-list').replaceChildren();
  }
  const history = byId('history');
  withFocusPreserved(history, () => {
    history.replaceChildren();
    if (S.global) { history.append(overview(act)); return; }
    if (!visibleMessages.length && !sessionRuns().length) { history.append(welcome()); return; }
    // Interleave messages with the Worker runs this Hub dispatched, by creation time where known.
    type Item = { at: string; node: () => HTMLElement };
    const items: Item[] = visibleMessages.map((m, i) => ({ at: m.created_at ?? String(i).padStart(8, '0'), node: () => messageNode(api, act, m) }));
    const runItems: Item[] = sessionRuns().map(r => ({ at: r.created_at, node: () => runCard(act, r) }));
    const merged = visibleMessages.every(m => m.created_at) ? [...items, ...runItems].sort((a, b) => a.at.localeCompare(b.at)) : [...items, ...runItems];
    for (const item of merged) history.append(item.node());
  });

  renderLive();
  if (atBottom) scroller.scrollTop = scroller.scrollHeight;

  // Composer chrome
  const prompt = byId<HTMLTextAreaElement>('prompt');
  prompt.disabled = S.global || !s;
  prompt.placeholder = S.global ? 'Choose a project Hub to send a message' : s ? 'Message ' + s.name + ' — Enter to send, Shift+Enter for a new line' : 'Register a project to start';
  byId<HTMLButtonElement>('send').disabled = S.global || !s;
  const pill = byId<HTMLButtonElement>('model-pill');
  pill.replaceChildren(h('span', { class: 'label', text: currentModelLabel() }), icon('chevronDown', 'ic--sm'));
  pill.disabled = !s || S.global;
  const verified = S.memory.filter(m => m.status === 'verified').length;
  byId('memory-chip').replaceChildren(icon('memory', 'ic--sm'), h('span', { text: 'Memory ' + verified }));
  const vendor = vendorOf(s ? s.profile_id + ' ' + s.provider_model : '');
  const usage = S.usage.find(u => vendorOf(u.vendor) === vendor);
  const ctx = usage?.context ?? null;
  byId('vial-label').textContent = (ctx ? compact(ctx) : '—') + ' / 512K';
  byId('vial-wrap').title = ctx ? 'Largest prompt in the last 24 hours: ' + ctx.toLocaleString() + ' of 512,000 tokens' : 'Context use not reported yet';
  document.dispatchEvent(new CustomEvent('pc-context', { detail: ctx ? ctx / 512000 : 0 }));
}

/** Update only the live turn (called on every delta, so it never rebuilds history). */
export function renderLive(): void {
  const live = S.selected && !S.global ? S.live.get(S.selected) : undefined;
  const box = byId('live');
  box.hidden = !live;
  byId('cancel').hidden = !live;
  if (!live) return;
  const think = byId<HTMLDetailsElement>('live-think');
  think.hidden = !live.reasoning;
  byId('live-reasoning').textContent = live.reasoning;
  byId('live-state').textContent = live.text ? 'Writing' : 'Thinking';
  const stream = byId('stream');
  if (stream.textContent !== live.text) stream.textContent = live.text;
}
