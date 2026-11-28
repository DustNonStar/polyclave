// Right inspector: Topology DAG, Review decisions, read-only Files and verified Memory.
import type { Api, Decision, RunDetail } from '../api/client.js';
import type { CouncilDto, PrGateDiffDto, PrGateDto, RunDto } from '../generated/proto.js';
import {
  S, activeRunDto, attentionPrGates, awaitingCouncils, needsReview, runLabel, runTone,
  selectedSession, totalNeedsCount, vendorLabel, visibleRuns,
} from '../state/store.js';
import type { Actions, Panel, ReviewView } from '../state/store.js';
import { withFocusPreserved } from '../state/focus.js';
import { button, byId, h, short, svgEl } from './elements.js';
import { icon } from './icons.js';
import type { IconName } from './icons.js';
import { dispatchWorker, openPrGateDialog, rememberRule, startCouncilDialog } from './flows.js';
import { areaField } from './dialog.js';
import { approvalEvidence, evidenceCard } from './evidence.js';

const PANELS: readonly [Panel, IconName][] = [['Topology', 'topology'], ['Review', 'review'], ['Files', 'file'], ['Memory', 'memory']];

export function renderTabs(act: Actions): void {
  const tabs = byId('inspect-tabs');
  if (!tabs.childElementCount) {
    PANELS.forEach(([name], i) => {
      const b = button(name, () => act.setPanel(name), 'tab', { role: 'tab', id: 'tab-' + name.toLowerCase(), 'aria-controls': 'inspect' });
      b.addEventListener('keydown', e => {
        const step = e.key === 'ArrowRight' ? 1 : e.key === 'ArrowLeft' ? -1 : 0;
        if (!step) return;
        const next = PANELS[(i + step + PANELS.length) % PANELS.length];
        if (next) { act.setPanel(next[0]); document.getElementById('tab-' + next[0].toLowerCase())?.focus(); }
      });
      tabs.append(b);
    });
  }
  const waiting = totalNeedsCount();
  for (const [name] of PANELS) {
    const b = byId('tab-' + name.toLowerCase());
    const on = name === S.panel;
    b.setAttribute('aria-selected', String(on)); b.tabIndex = on ? 0 : -1;
    b.replaceChildren(document.createTextNode(name));
    if (name === 'Review' && waiting) b.append(h('span', { class: 'badge', text: String(waiting), 'aria-label': waiting + ' waiting' }));
  }
}

export async function renderInspector(api: Api, act: Actions): Promise<void> {
  renderTabs(act);
  const which = S.panel;
  const panel = byId('inspect');
  panel.setAttribute('aria-labelledby', 'tab-' + which.toLowerCase());
  let nodes: Node[];
  if (which === 'Topology') nodes = topology(api, act);
  else if (which === 'Review') nodes = await review(api, act);
  else if (which === 'Files') nodes = files(api, act);
  else nodes = memory(api, act);
  if (S.panel !== which) return;
  withFocusPreserved(panel, () => panel.replaceChildren(...nodes));
  if (which === 'Topology') layoutTopology();
}

/* ---------- Topology ---------- */
function topology(api: Api, act: Actions): Node[] {
  const s = selectedSession();
  const runs = visibleRuns().slice(-8);
  const head = h('header', { class: 'panel-head' },
    h('h2', { text: S.global ? 'Agent network · all projects' : 'Agent network' }),
    h('small', { text: runs.length + ' worker' + (runs.length === 1 ? '' : 's') }));
  const topo = h('div', { class: 'topo', id: 'topo' });
  const wires = svgEl('svg', { class: 'topo-wires', id: 'topo-wires', 'aria-hidden': 'true' });
  topo.append(wires);
  const hub = h('button', { type: 'button', class: 'topo-node hub', 'data-node': 'hub', 'data-focus-key': 'topo-hub', onclick: () => act.setPane('chat') },
    h('div', { class: 'n-top' }, h('span', { class: 'vendor-tag', text: S.global ? 'Global' : 'Hub' }), h('span', { class: 'model', text: s ? (s.profile_id === 'builtin-hub' ? 'local' : s.provider_model) : '' })),
    h('div', { class: 'n-title', text: S.global ? 'Global Hub' : s?.name ?? 'No project selected' }),
    h('div', { class: 'n-foot' }, h('span', { class: 'dot ' + (s && S.live.has(s.id) ? 'dot--think pulse' : 'dot--idle') }), h('span', { text: s && S.live.has(s.id) ? 'Thinking' : 'Coordinating' })));
  topo.append(hub);
  for (const r of runs) {
    const tone = runTone(r.status);
    const pct = r.status === 'done' ? 100 : Math.round(100 * Math.min(1, r.current_attempt / Math.max(1, r.max_attempts)));
    topo.append(h('button', {
      type: 'button', class: 'topo-node', 'data-node': r.id, 'data-tone': tone, 'aria-pressed': String(r.id === S.activeRun), 'data-focus-key': 'topo-' + r.id,
      onclick: () => act.openRun(r.id, true), title: r.instructions.slice(0, 240),
    },
    h('div', { class: 'n-top' }, h('span', { class: 'vendor-tag', 'data-vendor': r.vendor, text: vendorLabel(r.vendor) }), h('span', { class: 'model', text: r.model ?? 'default' })),
    h('div', { class: 'n-title', text: r.instructions.split('\n')[0] || r.id }),
    h('div', { class: 'n-foot' }, h('span', { text: runLabel(r.status) }), h('span', { class: 'progress', 'data-tone': tone, style: '--value:' + pct + '%' }, h('i')), h('span', { text: r.current_attempt + '/' + r.max_attempts }))));
  }
  if (!runs.length) topo.append(h('p', { class: 'topo-empty', text: 'No Worker runs yet. Create a ticket from a message, then dispatch a Worker.' }));

  const nodes: Node[] = [head, topo];
  if (!S.global && s) {
    const tickets = S.tickets.filter(t => t.hub === s.id);
    const list = h('div', { class: 'ticket-list' });
    for (const t of tickets.slice(-6).reverse()) {
      const open = t.status === 'active' || t.status === 'proposed';
      list.append(h('div', { class: 'ticket-row' },
        icon('ticket', 'ic--sm'), h('span', { class: 'id', text: t.id }),
        h('span', { class: 'grow ellipsis', text: t.status.replaceAll('_', ' ') + ' · ' + t.used + '/' + t.limit + ' runs' }),
        open && t.used < t.limit ? button('Dispatch', () => dispatchWorker(api, act, t.id), undefined, { 'data-focus-key': 'disp-' + t.id }) : null));
    }
    nodes.push(h('header', { class: 'panel-head' }, h('h2', { text: 'Tickets' }),
      button([icon('bolt', 'ic--sm'), 'Dispatch Worker'], () => dispatchWorker(api, act, null), 'btn-ghost', { 'data-focus-key': 'disp-free' })));
    nodes.push(tickets.length ? list : h('p', { class: 'empty', text: 'No tickets. Use “Create ticket” on one of your messages.' }));
  }
  const waiting = runs.find(r => r.status === 'needs_review');
  if (waiting) {
    const sha = waiting.observed_sha ?? waiting.result_sha;
    nodes.push(h('article', { class: 'card', 'data-tone': 'review' },
      h('div', { class: 'row' }, h('span', { class: 'card-title grow', text: waiting.id + ' · waiting for your decision' }), h('span', { class: 'vendor-tag', 'data-vendor': waiting.vendor, text: waiting.vendor })),
      h('p', { class: 'card-sub', text: (waiting.detail ?? 'Host verification finished.') + (sha ? ' Result ' + short(sha) + '.' : '') }),
      h('div', { class: 'actions' }, button([icon('review', 'ic--sm'), 'Open review'], () => act.openRun(waiting.id, true), 'btn-primary', { 'data-focus-key': 'quick-review' }))));
  }
  return nodes;
}

let observer: ResizeObserver | null = null;
export function layoutTopology(): void {
  const topo = document.getElementById('topo');
  if (!topo) return;
  if (!observer) observer = new ResizeObserver(() => layoutTopology());
  observer.disconnect(); observer.observe(topo.parentElement ?? topo);
  const W = topo.clientWidth || 380;
  const pad = 12, gap = 12;
  const hub = topo.querySelector<HTMLElement>('[data-node="hub"]');
  const workers = [...topo.querySelectorAll<HTMLElement>('.topo-node:not(.hub)')];
  const hubW = Math.min(240, W - pad * 2), hubH = 78;
  const hx = (W - hubW) / 2, hy = pad;
  if (hub) Object.assign(hub.style, { left: hx + 'px', top: hy + 'px', width: hubW + 'px', height: hubH + 'px' });
  const cols = W >= 330 ? 2 : 1;
  const nodeW = (W - pad * 2 - gap * (cols - 1)) / cols, nodeH = 92, rowGap = 40;
  const top0 = hy + hubH + 46;
  const wires = topo.querySelector('#topo-wires');
  wires?.replaceChildren();
  const startX = W / 2, startY = hy + hubH;
  workers.forEach((n, i) => {
    const col = i % cols, row = Math.floor(i / cols);
    // Centre a lone last node.
    const lone = cols === 2 && i === workers.length - 1 && workers.length % 2 === 1;
    const x = lone ? (W - nodeW) / 2 : pad + col * (nodeW + gap);
    const y = top0 + row * (nodeH + rowGap);
    Object.assign(n.style, { left: x + 'px', top: y + 'px', width: nodeW + 'px', height: nodeH + 'px' });
    if (wires) {
      const ex = x + nodeW / 2, ey = y;
      // First row: a direct Bézier. Lower rows drop down the centre trunk between columns, then curve in.
      const d = row === 0
        ? `M${startX} ${startY} C${startX} ${startY + (ey - startY) * 0.55} ${ex} ${ey - (ey - startY) * 0.55} ${ex} ${ey}`
        : `M${startX} ${startY} L${startX} ${ey - 26} C${startX} ${ey - 10} ${ex} ${ey - 22} ${ex} ${ey}`;
      wires.append(svgEl('path', { d, 'data-tone': n.dataset.tone ?? '' }));
      wires.append(svgEl('circle', { cx: ex, cy: ey, r: 2.5 }));
    }
  });
  if (wires && workers.length) wires.append(svgEl('circle', { cx: startX, cy: startY, r: 3 }));
  const rows = Math.ceil(workers.length / cols);
  const height = workers.length ? top0 + rows * nodeH + (rows - 1) * rowGap + pad : hy + hubH + 70;
  topo.style.height = height + 'px';
  const empty = topo.querySelector<HTMLElement>('.topo-empty');
  if (empty) { empty.style.position = 'absolute'; empty.style.left = '0'; empty.style.right = '0'; empty.style.top = (hy + hubH + 8) + 'px'; }
}

/* ---------- Review ---------- */
let confirmAccept: string | null = null;

function parseFiles(files: string): { path: string; add: string; del: string }[] {
  return files.split('\n').map(l => l.trim()).filter(Boolean).map(line => {
    const tab = line.split('\t');
    if (tab.length >= 3) return { add: tab[0] ?? '', del: tab[1] ?? '', path: tab.slice(2).join('\t') };
    const stat = /^(.*?)\s*\|\s*(\d+)\s*([+-]*)$/.exec(line);
    if (stat) { const plus = (stat[3] ?? '').split('+').length - 1; const minus = (stat[3] ?? '').length - plus; return { path: (stat[1] ?? '').trim(), add: plus ? String(plus) : '', del: minus ? String(minus) : '' }; }
    return { path: line, add: '', del: '' };
  });
}

function reviewSubtabs(api: Api, act: Actions): HTMLElement {
  const views: [ReviewView, string, number][] = [
    ['runs', 'Runs', needsReview().length],
    ['council', 'Council', awaitingCouncils().length],
    ['pr_gates', 'PR Gates', attentionPrGates().length],
  ];
  const bar = h('div', { class: 'review-subtabs', role: 'group', 'aria-label': 'Review sections' });
  for (const [key, label, count] of views) {
    const b = button(label, () => { S.reviewView = key; void renderInspector(api, act); }, 'subtab', {
      'aria-pressed': String(S.reviewView === key),
      'data-review-view': key,
      'data-focus-key': 'rev-sub-' + key,
    });
    if (count > 0) b.append(h('span', { class: 'badge', text: String(count) }));
    bar.append(b);
  }
  return bar;
}

function councilTone(status: CouncilDto['status']): string {
  switch (status) {
    case 'running': return 'run';
    case 'awaiting_user': return 'review';
    case 'decided': return 'done';
    case 'failed': return 'fail';
    default: return 'idle';
  }
}

function prGateTone(status: PrGateDto['status']): string {
  switch (status) {
    case 'reviewing': return 'run';
    case 'awaiting_human_approval': return 'review';
    case 'scope_violation': case 'revise_requested': case 'escalated': case 'paused': return 'review';
    case 'approved': case 'auto_merge_armed': case 'merged': return 'done';
    case 'failed': return 'fail';
    default: return 'idle';
  }
}

function councilSection(api: Api, act: Actions): Node[] {
  const list = S.councils;
  const active = list.find(c => c.id === S.activeCouncil) ?? list.find(c => c.status === 'awaiting_user') ?? list.slice(-1)[0];
  const head = h('header', { class: 'panel-head' },
    h('h2', { text: 'Blind Council' }),
    button([icon('bolt', 'ic--sm'), 'New Council'], () => startCouncilDialog(api, act), 'btn-ghost', { 'data-focus-key': 'new-council', disabled: S.pause.paused }));
  if (!active) {
    return [head, h('p', { class: 'empty', text: 'No blind councils yet. Start one to gather anonymous proposals from enabled managers.' })];
  }
  const nodes: Node[] = [head];
  if (list.length > 1) {
    const picker = h('select', { class: 'field run-picker', 'aria-label': 'Council session', 'data-focus-key': 'council-picker' });
    for (const c of list.slice().reverse()) {
      picker.add(new Option(c.id + ' · ' + c.status.replaceAll('_', ' ') + ' · ' + c.prompt.slice(0, 44), c.id));
    }
    picker.value = active.id;
    picker.addEventListener('change', () => { S.activeCouncil = picker.value; void renderInspector(api, act); });
    nodes.push(picker);
  }

  const meta = h('section', { class: 'review-head' },
    h('div', { class: 'row' },
      h('span', { class: 'eyebrow grow', text: active.id + ' · ' + active.level }),
      h('span', { class: 'pill-tag', 'data-tone': councilTone(active.status), text: active.status.replaceAll('_', ' ') })),
    h('h2', { text: active.prompt }),
    active.escalation_reasons.length
      ? h('div', { class: 'chips' }, ...active.escalation_reasons.map(r => h('span', { class: 'pill-tag', 'data-tone': 'review', text: r })))
      : null);
  nodes.push(meta);

  const grid = h('div', { class: 'council-grid' });
  let selectedLabel = active.chosen_label ?? active.proposals[0]?.label ?? '';
  for (const p of active.proposals) {
    const revealed = active.status === 'decided' && p.vendor;
    const isChosen = active.chosen_label === p.label;
    const card = h('article', {
      class: 'card proposal-card',
      'data-chosen': String(isChosen),
      'data-blind': String(!revealed),
    },
    h('div', { class: 'row' },
      h('span', { class: 'card-title grow', text: p.label }),
      revealed
        ? h('span', { class: 'vendor-tag', 'data-vendor': p.vendor ?? undefined, text: vendorLabel(p.vendor!) + (p.model ? ' · ' + p.model : '') })
        : h('span', { class: 'pill-tag', text: 'Blind' })),
    h('p', { class: 'proposal-summary', text: p.payload.summary }),
    h('p', { class: 'card-sub', text: p.payload.approach }),
    h('div', { class: 'risk-chips' },
      h('span', { class: 'pill-tag', text: p.payload.touched_paths.length + ' paths' }),
      p.payload.security_sensitive ? h('span', { class: 'pill-tag', 'data-tone': 'review', text: 'security sensitive' }) : null,
      p.payload.public_interface_changes ? h('span', { class: 'pill-tag', 'data-tone': 'review', text: 'public API change' }) : null,
      p.payload.new_dependencies.length ? h('span', { class: 'pill-tag', 'data-tone': 'review', text: p.payload.new_dependencies.length + ' new deps' }) : null,
      p.payload.beyond_request ? h('span', { class: 'pill-tag', 'data-tone': 'review', text: 'beyond request' }) : null,
      p.payload.open_questions.length ? h('span', { class: 'pill-tag', 'data-tone': 'review', text: p.payload.open_questions.length + ' open questions' }) : null),
    p.payload.pros.length ? h('p', { class: 'decision-note', text: 'Pros: ' + p.payload.pros.join(' · ') }) : null,
    p.payload.cons.length ? h('p', { class: 'decision-note', text: 'Cons: ' + p.payload.cons.join(' · ') }) : null);
    grid.append(card);
  }
  nodes.push(grid);

  if (active.status === 'awaiting_user' && active.proposals.length) {
    const [noteL, note] = areaField('Decision note', { rows: 2, placeholder: 'Optional rationale for choosing this proposal.' });
    note.dataset.focusKey = 'council-note-' + active.id;
    const choiceBar = h('div', { class: 'actions' });
    const btns: HTMLButtonElement[] = [];
    for (const p of active.proposals) {
      const b = button('Choose ' + p.label, () => {
        selectedLabel = p.label;
        for (const x of btns) x.setAttribute('aria-pressed', String(x === b));
      }, 'btn-ghost', { 'aria-pressed': String(selectedLabel === p.label), 'data-focus-key': 'choose-' + p.label });
      btns.push(b);
      choiceBar.append(b);
    }
    const confirm = button([icon('check'), 'Confirm decision'], () => {
      if (!selectedLabel) return;
      confirm.disabled = true;
      void api.resolveCouncil(active.id, selectedLabel, note.value.trim() || null).then(async updated => {
        S.activeCouncil = updated.id;
        act.tell('Council decided: ' + selectedLabel, 'info');
        await act.refresh();
      }).catch(e => { confirm.disabled = false; act.problem(e); });
    }, 'btn-primary', { 'data-focus-key': 'confirm-council' });
    nodes.push(h('section', { class: 'card decision', 'data-tone': 'review' },
      h('span', { class: 'card-title', text: 'Select winning proposal' }),
      choiceBar, noteL, h('div', { class: 'actions' }, confirm)));
  } else if (active.decision_note) {
    nodes.push(h('p', { class: 'decision-note', text: 'Decision note: ' + active.decision_note }));
  }
  return nodes;
}

function safeHttpsUrl(raw: string | null | undefined): string | null {
  if (!raw) return null;
  try {
    const parsed = new URL(raw);
    return parsed.protocol === 'https:' ? parsed.href : null;
  } catch {
    return null;
  }
}

/** The mirror diff for the gate's exact SHA, cached by gate id + head SHA. */
function approvalDiffSlot(api: Api, act: Actions, gate: PrGateDto): HTMLElement {
  const diffKey = gate.id + ':' + gate.head_sha;
  const slot = h('div', { class: 'approval-diff', 'data-diff-sha': gate.head_sha });
  const cached = S.prGateDiffs.get(diffKey);
  if (cached) {
    slot.append(diffPanel(cached, gate.pr_url));
    return slot;
  }
  const load = button('Show the diff for ' + short(gate.head_sha), () => {
    load.disabled = true;
    void api.prGateDiff(gate.id).then(diff => {
      S.prGateDiffs.set(diffKey, diff);
      slot.replaceChildren(diffPanel(diff, gate.pr_url));
    }).catch((e: unknown) => {
      load.disabled = false;
      act.tell(e instanceof Error ? e.message : 'The diff could not be read', 'info');
    });
  }, 'btn-ghost', { 'data-focus-key': 'show-diff' });
  slot.append(load);
  return slot;
}

function diffPanel(diff: PrGateDiffDto, prUrl?: string | null): HTMLElement {
  const allLines = diff.text.split('\n');
  const lineTruncated = allLines.length > 400;
  const body = h('pre', { class: 'diff-body', text: allLines.slice(0, 400).join('\n') });
  let expanded = false;
  const toggle = lineTruncated
    ? button('Show all ' + allLines.length + ' lines', () => {
        expanded = !expanded;
        body.textContent = (expanded ? allLines : allLines.slice(0, 400)).join('\n');
        if (toggle) toggle.textContent = expanded ? 'Show first 400 lines' : 'Show all ' + allLines.length + ' lines';
      }, 'btn-ghost', { 'data-focus-key': 'toggle-full-diff' })
    : null;
  const safePr = safeHttpsUrl(prUrl);
  return h('div', { class: 'diff-panel', 'data-diff': 'text' },
    h('div', { class: 'diff-title', text: short(diff.base_sha) + ' .. ' + short(diff.head_sha) + ' · ' + diff.bytes + ' bytes' }),
    diff.truncated || lineTruncated
      ? h('p', { class: 'field-hint', 'data-truncated': 'true', text: (lineTruncated && !diff.truncated ? 'Showing first 400 of ' + allLines.length + ' lines. ' : 'Showing the first ' + diff.text.length + ' of ' + diff.bytes + ' bytes. ') + 'Full diff is never silently hidden.' },
          toggle,
          safePr ? h('a', { href: safePr, target: '_blank', rel: 'noopener noreferrer', class: 'btn-ghost', text: 'Open PR on GitHub' }) : null)
      : null,
    body);
}

function prGateEvidenceSlot(gate: PrGateDto): HTMLElement | null {
  const detail = S.details.get(gate.run_id);
  const ev = detail?.evidence;
  const tests = (ev?.test_results ?? []).filter(t => t.sha === gate.head_sha);
  const notDone = ev && (ev.receipt_sha === gate.head_sha || ev.test_sha === gate.head_sha)
    ? ev.receipt_not_done?.trim() || null
    : null;
  const box = h('div', { class: 'diff-well', 'data-gate-evidence': gate.head_sha },
    h('div', { class: 'diff-title', text: 'Evidence for ' + short(gate.head_sha) }));
  if (tests.length) {
    for (const t of tests) {
      box.append(h('div', { class: 'diff-row' },
        h('span', { class: 'pill-tag', 'data-tone': t.status === 'passed' ? 'done' : t.status === 'failed' ? 'fail' : 'review', text: t.status }),
        h('span', { class: 'path', text: (t.source === 'ci' ? 'CI: ' : t.source === 'host_run' ? 'Host: ' : 'Claim: ') + (t.name || t.summary) })));
    }
  } else {
    box.append(h('div', { class: 'diff-row' }, h('span', { class: 'path', text: 'No test result recorded for ' + short(gate.head_sha) + '.' })));
  }
  box.append(h('div', { class: 'diff-row' },
    h('span', { class: 'path', text: notDone ? 'Not done (Worker): ' + notDone : 'No unfinished items listed by Worker.' })));
  return box;
}

function prGatesSection(api: Api, act: Actions): Node[] {
  const list = S.prGates;
  const head = h('header', { class: 'panel-head' },
    h('h2', { text: 'PR Gates & SCM Observer' }),
    h('small', { text: S.pause.paused ? 'Globally paused' : list.length + ' gates' }));
  const active = list.find(g => g.id === S.activePrGate) ?? attentionPrGates()[0] ?? list.slice(-1)[0];
  if (!active) {
    return [head, h('p', { class: 'empty', text: 'No PR gates yet. Completed Worker runs can be pushed and reviewed through PR Gate.' })];
  }
  const nodes: Node[] = [head];
  if (list.length > 1) {
    const picker = h('select', { class: 'field run-picker', 'aria-label': 'PR Gate', 'data-focus-key': 'pr-gate-picker' });
    for (const g of list.slice().reverse()) {
      picker.add(new Option(g.id + ' · ' + g.status.replaceAll('_', ' ') + ' · ' + g.repo_slug, g.id));
    }
    picker.value = active.id;
    picker.addEventListener('change', () => { S.activePrGate = picker.value; void renderInspector(api, act); });
    nodes.push(picker);
  }
  const safePr = safeHttpsUrl(active.pr_url);
  const canRunGate = !active.auto_merge_armed && (
    active.status === 'failed'
    || active.status === 'escalated'
    || active.status === 'scope_violation'
    || active.status === 'paused'
  );
  const gateCard = h('section', { class: 'card pr-gate-card', 'data-tone': prGateTone(active.status) },
    h('div', { class: 'row' },
      h('span', { class: 'card-title grow', text: active.repo_slug + (active.pr_number ? ' #' + active.pr_number : '') }),
      safePr ? h('a', { href: safePr, target: '_blank', rel: 'noopener noreferrer', class: 'pill-tag', 'data-pr-link': 'true', text: 'GitHub PR' }) : null,
      h('span', { class: 'pill-tag', 'data-tone': prGateTone(active.status), text: active.status.replaceAll('_', ' ') })),
    h('div', { class: 'chips' },
      h('span', { class: 'sha' }, h('b', { text: 'branch' }), active.branch),
      h('span', { class: 'sha' }, h('b', { text: 'base' }), short(active.base_sha)),
      h('span', { class: 'sha', 'data-tone': 'done' }, h('b', { text: 'head' }), short(active.head_sha)),
      active.auto_merge_armed ? h('span', { class: 'pill-tag', 'data-tone': 'done', text: 'auto-merge armed' }) : null,
      active.revise_rounds > 0 ? h('span', { class: 'pill-tag', text: active.revise_rounds + ' revise rounds' }) : null),
    active.scope_violations.length
      ? h('div', { class: 'diff-well' },
          h('div', { class: 'diff-title', text: 'Scope violations (' + active.scope_violations.length + ')' }),
          ...active.scope_violations.map(v => h('div', { class: 'diff-row' }, h('span', { class: 'path', text: v }))))
      : null,
    prGateEvidenceSlot(active),
    // Exactly the SHA a person is being asked to approve, and what it contains.
    active.status === 'awaiting_human_approval'
      ? h('div', { class: 'diff-well', 'data-approval': 'pending' },
          h('div', { class: 'diff-title', text: 'Waiting for your approval of ' + short(active.head_sha) }),
          h('p', { class: 'field-hint', text: 'Reviewers approved this exact commit. Auto-merge is not armed and nothing is sent to GitHub until you approve this same SHA.' }),
          approvalDiffSlot(api, act, active))
      : null,
    h('div', { class: 'actions' },
      active.status === 'awaiting_human_approval'
        ? button([icon('check', 'ic--sm'), 'Approve ' + short(active.head_sha)], () => {
            void api.approvePrGate(active.id, active.head_sha).then(async () => {
              act.tell('Approved ' + short(active.head_sha), 'info');
              await act.refresh();
            }).catch((e: unknown) => {
              // A gate whose head moved refuses the old SHA: say so, do not retry.
              act.tell(e instanceof Error ? e.message : 'The approval was refused', 'info');
              void act.refresh();
            });
          }, 'btn-primary', { 'data-focus-key': 'approve-pr-gate', disabled: S.pause.paused })
        : null,
      canRunGate
        ? button([icon('review', 'ic--sm'), 'Run PR Gate'], () => {
            void api.runPrGate(active.id).then(async () => {
              act.tell('PR Gate started', 'info');
              await act.refresh();
            }).catch(act.problem);
          }, 'btn-primary', { 'data-focus-key': 'run-pr-gate', disabled: S.pause.paused })
        : null));
  nodes.push(gateCard);

  if (active.reviews.length) {
    const revBox = h('section', { class: 'diff-well' }, h('div', { class: 'diff-title', text: 'Heterogeneous reviews (' + active.reviews.length + ')' }));
    for (const rv of active.reviews) {
      revBox.append(h('div', { class: 'diff-row' },
        h('span', { class: 'vendor-tag', 'data-vendor': rv.reviewer_vendor, text: vendorLabel(rv.reviewer_vendor) }),
        h('span', { class: 'path', text: rv.reviewer_model + ' · ' + rv.verdict + ' @ ' + short(rv.reviewed_sha) + (rv.comments ? ' — ' + rv.comments : '') })));
    }
    nodes.push(revBox);
  }
  return nodes;
}

async function review(api: Api, act: Actions): Promise<Node[]> {
  const sub = reviewSubtabs(api, act);
  if (S.reviewView === 'council') return [sub, ...councilSection(api, act)];
  if (S.reviewView === 'pr_gates') {
    const activeGate = S.prGates.find(g => g.id === S.activePrGate) ?? attentionPrGates()[0] ?? S.prGates.slice(-1)[0];
    if (activeGate) {
      try {
        const detail = await api.run(activeGate.run_id);
        S.details.set(activeGate.run_id, detail);
      } catch { /* keep cached detail */ }
    }
    return [sub, ...prGatesSection(api, act)];
  }
  const runs = visibleRuns();
  const run = activeRunDto();
  if (!run) return [sub, h('p', { class: 'empty', text: 'No Worker run selected. Dispatch a Worker from a ticket to see host verification here.' })];
  let detail: RunDetail | undefined = S.details.get(run.id);
  try { detail = await api.run(run.id); S.details.set(run.id, detail); } catch { /* keep the last known detail */ }
  const r: RunDto = detail?.run ?? run;
  const tone = runTone(r.status);
  const sha = r.observed_sha ?? r.result_sha;

  const picker = h('select', { class: 'field run-picker', 'aria-label': 'Worker run', 'data-focus-key': 'run-picker' });
  for (const x of runs.slice().reverse()) picker.add(new Option(x.id + ' · ' + runLabel(x.status) + ' · ' + (x.instructions.split('\n')[0] ?? '').slice(0, 50), x.id));
  picker.value = r.id;
  picker.addEventListener('change', () => act.openRun(picker.value));

  const head = h('section', { class: 'review-head' },
    h('div', { class: 'row' }, h('span', { class: 'eyebrow grow', text: r.id + ' · attempt ' + r.current_attempt + '/' + r.max_attempts }), h('span', { class: 'vendor-tag', 'data-vendor': r.vendor, text: vendorLabel(r.vendor) + (r.model ? ' · ' + r.model : '') })),
    h('h2', { text: r.instructions.split('\n')[0] || r.id }),
    h('div', { class: 'chips' },
      h('span', { class: 'pill-tag', 'data-tone': tone, text: runLabel(r.status) }),
      h('span', { class: 'sha' }, h('b', { text: 'base' }), short(r.base_sha)),
      r.observed_sha ? h('span', { class: 'sha', 'data-tone': 'review' }, h('b', { text: 'observed' }), short(r.observed_sha)) : null,
      r.result_sha ? h('span', { class: 'sha', 'data-tone': 'done' }, h('b', { text: 'result' }), short(r.result_sha)) : null),
    r.detail ? h('p', { class: 'card-sub', text: r.detail }) : null,
    r.resolution ? h('p', { class: 'card-sub', text: 'Resolution: ' + r.resolution }) : null);

  const evidence = approvalEvidence(r, detail?.evidence);
  const nodes: Node[] = [sub, runs.length > 1 ? picker : h('span'), head, evidenceCard(evidence)];

  const diff = h('section', { class: 'diff-well' }, h('div', { class: 'diff-title' }, icon('branch', 'ic--sm'), 'What changed', sha ? h('span', { class: 'sha', text: short(sha) }) : null));
  if (detail?.commit) {
    for (const c of detail.commit.commits.split('\n').filter(Boolean)) {
      const [hash, ...rest] = c.split(' ');
      diff.append(h('div', { class: 'diff-row commit' }, h('span', { class: 'hash', text: (hash ?? '').slice(0, 7) }), h('span', { class: 'path', text: rest.join(' ') || c })));
    }
    for (const f of parseFiles(detail.commit.files)) {
      diff.append(h('div', { class: 'diff-row' }, h('span', { class: 'path', text: f.path }), f.add ? h('span', { class: 'add', text: '+' + f.add }) : null, f.del ? h('span', { class: 'del', text: '−' + f.del }) : null));
    }
  } else diff.append(h('div', { class: 'diff-row' }, h('span', { class: 'path', text: r.status === 'running' || r.status === 'pending' ? 'The Worker has not produced a result yet.' : 'No commit evidence recorded for this run.' })));
  nodes.push(diff);

  // Decision
  const decision = h('section', { class: 'card decision', 'data-tone': tone });
  const reviewable = r.status === 'needs_review' || r.status === 'interrupted';
  if (reviewable) {
    const [noteL, note] = areaField('Decision note', { rows: 2, placeholder: 'Required for Mark failed and Allow retry. Example: tests fail on Windows path handling.' });
    note.dataset.focusKey = 'decision-note-' + r.id;
    const err = h('p', { class: 'field-hint', role: 'alert' });
    err.style.color = 'var(--red)';
    const resolve = (d: Decision) => {
      const text = note.value.trim();
      if (d !== 'accept' && !text) { err.textContent = 'Write a note so the next attempt knows what to change.'; note.focus(); return; }
      void api.resolve(r.id, d, text, d === 'accept' ? sha : null).then(async () => {
        confirmAccept = null;
        act.tell(d === 'accept' ? 'Accepted ' + short(sha) : d === 'fail' ? 'Run marked failed' : 'Retry allowed', 'info');
        await act.refresh();
        if (d !== 'accept' && text) rememberRule(api, act, text, { run_id: r.id });
      }).catch(act.problem);
    };
    const acts = h('div', { class: 'actions' });
    if (sha && confirmAccept === r.id) {
      acts.append(
        button([icon('check'), 'Confirm accept ' + short(sha)], () => resolve('accept'), 'btn-primary', { 'data-focus-key': 'confirm-accept' }),
        button('Back', () => { confirmAccept = null; void renderInspector(api, act); }, 'btn-ghost', { 'data-focus-key': 'accept-back' }));
    } else {
      if (sha) acts.append(button([icon('check'), 'Accept ' + short(sha)], () => { confirmAccept = r.id; void renderInspector(api, act).then(() => document.querySelector<HTMLElement>('[data-focus-key="confirm-accept"]')?.focus()); }, 'btn-primary', { 'data-focus-key': 'accept' }));
      acts.append(button('Mark failed', () => resolve('fail'), 'btn-danger', { 'data-focus-key': 'fail' }), button('Allow retry', () => resolve('allow_retry'), 'btn-ghost', { 'data-focus-key': 'allow-retry' }));
    }
    decision.append(h('div', { class: 'row' }, h('span', { class: 'card-title grow', text: sha ? 'Your decision on ' + short(sha) : 'Your decision' })), noteL, err, acts,
      h('p', { class: 'decision-note', text: sha ? 'Accept applies only to ' + short(sha) + ', the SHA shown here. Allow retry supersedes this attempt and starts a fresh branch on the next run.' : 'No result SHA was observed, so this run can only be failed or retried.' }));
  } else {
    decision.append(h('div', { class: 'row' }, h('span', { class: 'card-title grow', text: r.status === 'done' ? 'Accepted and verified' : r.status === 'failed' ? 'Closed as failed' : 'Worker in progress' })));
  }
  const ops = h('div', { class: 'actions' });
  if (r.status === 'running' || r.status === 'pending') ops.append(button([icon('stop', 'ic--sm'), 'Cancel'], () => { void api.cancelRun(r.id).then(() => { act.tell('Cancel requested', 'info'); return act.refresh(); }).catch(act.problem); }, 'btn-danger', { 'data-focus-key': 'run-cancel' }));
  if (r.status === 'failed' || r.status === 'interrupted') ops.append(button([icon('retry', 'ic--sm'), 'Retry'], () => { void api.retry(r.id).then(() => { act.tell('Retry scheduled', 'info'); return act.refresh(); }).catch(act.problem); }, 'btn-ghost', { 'data-focus-key': 'run-retry' }));
  if (r.status === 'done' && r.result_source === 'agent' && r.result_sha) {
    const runGates = S.prGates.filter(g => g.run_id === r.id);
    const activeOrMergedGate = runGates.find(g =>
      g.status !== 'failed' && g.status !== 'escalated' && g.status !== 'scope_violation',
    );
    const latestGate = runGates[runGates.length - 1];
    if (latestGate) {
      ops.append(button([icon('review', 'ic--sm'), 'View PR Gate'], () => {
        S.activePrGate = (activeOrMergedGate ?? latestGate).id;
        S.reviewView = 'pr_gates';
        act.setPanel('Review');
      }, 'btn-ghost', { 'data-focus-key': 'view-pr-gate' }));
    }
    if (!activeOrMergedGate) {
      ops.append(button([icon('review', 'ic--sm'), 'Open PR Gate'], () => openPrGateDialog(api, act, r), 'btn-primary', { 'data-focus-key': 'open-pr-gate', disabled: S.pause.paused }));
    }
  }
  if (ops.childElementCount) decision.append(ops);
  nodes.push(decision);

  // Follow-up chat with the Worker
  if (r.status !== 'done' && r.status !== 'failed') {
    const [chatL, chat] = areaField('Ask the Worker', { rows: 2, placeholder: 'Follow-up question or correction for this attempt' });
    chat.dataset.focusKey = 'worker-chat-' + r.id;
    const live = S.live.get(r.id);
    nodes.push(h('section', { class: 'card chat-box' }, h('div', { class: 'row' }, icon('chat', 'ic--sm'), h('span', { class: 'card-title', text: 'worker-chat' })),
      live?.text ? h('p', { class: 'card-sub mono', text: live.text.slice(-400) }) : null,
      chatL, h('div', { class: 'row chat-send' }, button([icon('send', 'ic--sm'), 'Send to Worker'], () => {
        const text = chat.value.trim();
        if (!text) return;
        void api.workerChat(r.id, text).then(() => { chat.value = ''; act.tell('Sent to ' + r.id, 'info'); }).catch(act.problem);
      }, 'btn-ghost', { 'data-focus-key': 'worker-chat-send' }))));
  }
  return nodes;
}

/* ---------- Files ---------- */
function files(api: Api, act: Actions): Node[] {
  const s = selectedSession();
  if (!s) return [h('p', { class: 'empty', text: 'Choose a project Hub to browse its registered repository.' })];
  const input = h('input', { type: 'text', value: S.filePath, placeholder: 'src/main.rs', 'aria-label': 'Relative file path', 'data-focus-key': 'file-path', spellcheck: false });
  const read = () => {
    const path = input.value.trim();
    if (!path) return;
    S.filePath = path;
    void api.repoFile(s.id, path).then(v => { S.fileText = v.text; S.fileError = ''; act.rerender(); })
      .catch(e => { S.fileText = null; S.fileError = e instanceof Error ? e.message : 'The file could not be read'; act.rerender(); });
  };
  input.addEventListener('keydown', e => { if (e.key === 'Enter') { e.preventDefault(); read(); } });
  const form = h('div', { class: 'file-form' }, input, button([icon('file', 'ic--sm'), 'Read'], read, 'btn-ghost', { 'data-focus-key': 'file-read' }));
  const nodes: Node[] = [h('header', { class: 'panel-head' }, h('h2', { text: 'Project files' }), h('small', { text: 'read-only' })), form];
  if (S.fileError) nodes.push(h('p', { class: 'empty', role: 'alert', text: S.fileError }));
  if (S.fileText !== null) {
    const ol = h('ol');
    for (const line of S.fileText.split('\n').slice(0, 4000)) ol.append(h('li', { text: line || ' ' }));
    nodes.push(h('div', { class: 'file-view', tabindex: '0', 'aria-label': S.filePath }, ol));
  } else if (!S.fileError) nodes.push(h('p', { class: 'empty', text: 'Paths resolve inside ' + s.repo_cwd.split(/[\\/]/).pop() + '; files outside the registered repository are refused.' }));
  return nodes;
}

/* ---------- Memory ---------- */
function memory(api: Api, act: Actions): Node[] {
  const s = selectedSession();
  if (!s) return [h('p', { class: 'empty', text: 'Choose a project Hub to see its verified memory.' })];
  const kinds = ['all', ...new Set(S.memory.map(m => m.kind))];
  const filters = h('div', { class: 'mem-filters', role: 'group', 'aria-label': 'Filter by kind' });
  for (const k of kinds) filters.append(button(k.replaceAll('_', ' '), () => { S.memFilter = k; act.rerender(); }, undefined, { 'aria-pressed': String(S.memFilter === k), 'data-focus-key': 'mem-' + k }));
  const items = S.memory.filter(m => m.status !== 'archived' && (S.memFilter === 'all' || m.kind === S.memFilter));
  const nodes: Node[] = [
    h('header', { class: 'panel-head' }, h('h2', { text: 'Verified memory' }), h('small', { text: S.memory.filter(m => m.status === 'verified').length + ' verified' })),
    h('p', { class: 'card-sub', text: 'Rules carry across providers. Use “Remember this rule” on a message or a review decision; drafts become active only after you confirm.' }),
    filters,
  ];
  if (!items.length) nodes.push(h('p', { class: 'empty', text: 'No memory in this view yet.' }));
  for (const m of items) {
    const tone = m.status === 'verified' ? 'done' : m.status === 'candidate' ? 'review' : 'archive';
    nodes.push(h('article', { class: 'card mem-item' },
      h('div', { class: 'row' }, h('span', { class: 'card-title grow', text: m.title }),
        m.status === 'verified' ? button('Archive', () => { void api.archiveMemory(s.id, m.id).then(() => act.refresh()).catch(act.problem); }, 'btn-ghost', { 'data-focus-key': 'arch-' + m.id }) : null),
      h('p', { text: m.content }),
      h('div', { class: 'mem-meta' },
        h('span', { class: 'pill-tag', 'data-tone': tone, text: m.status }),
        h('span', { class: 'pill-tag', text: m.kind.replaceAll('_', ' ') }),
        h('span', { class: 'pill-tag', text: m.scope }),
        ...m.tags.slice(0, 4).map(t => h('span', { class: 'pill-tag', text: '#' + t })))));
  }
  return nodes;
}
