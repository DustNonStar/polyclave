// Dialog flows that create durable records. Each one shows the exact data and saves only on explicit confirmation.
import type { Api } from '../api/client.js';
import type { CouncilLevel, MemoryKind, PlanningPreference, RunDto, Vendor } from '../generated/proto.js';
import { S, selectedSession } from '../state/store.js';
import type { Actions } from '../state/store.js';
import { button, h } from './elements.js';
import { areaField, closeDialog, openDialog, segmented, selectField, textField } from './dialog.js';
import { icon } from './icons.js';

const KINDS: readonly MemoryKind[] = ['constraint', 'preference', 'pitfall', 'decision', 'codebase_fact'];
const lines = (v: string) => v.split(/[\n,]/).map(s => s.trim()).filter(Boolean);

function errorLine(): HTMLElement { const p = h('p', { class: 'field-hint', role: 'alert' }); p.style.color = 'var(--red)'; return p; }

export function rememberRule(api: Api, act: Actions, text: string, source: { message_id: number } | { run_id: string }): void {
  const session = S.selected;
  if (!session) return;
  const [titleL, title] = textField('Rule title', { value: text.split('\n')[0]?.slice(0, 64) || 'Project rule' });
  const [ruleL, rule] = areaField('Rule text', { value: text, rows: 5 });
  const [kindL, kind] = selectField('Kind', KINDS, 'constraint');
  const err = errorLine();
  const save = button([icon('check'), 'Confirm and save'], () => {
    const t = title.value.trim(), c = rule.value.trim();
    if (!t || !c) { err.textContent = 'A title and rule text are required.'; return; }
    save.disabled = true;
    void (async () => {
      const draft = await api.draftMemory(session, kind.value as MemoryKind, t, c, source);
      await api.commitMemory(session, draft.id);
      closeDialog();
      act.tell('Verified rule saved', 'info');
      act.setPanel('Memory');
      await act.refresh();
    })().catch(e => { save.disabled = false; err.textContent = e instanceof Error ? e.message : 'The rule was not saved'; });
  }, 'btn-primary');
  openDialog('Remember this rule', [
    h('p', { text: 'Check the wording. The rule is drafted and then verified only when you confirm; until then nothing is applied to future turns.' }),
    titleL, ruleL, kindL,
    h('p', { class: 'field-hint', text: 'Scope: this project. Earlier versions are kept as superseded records.' }), err,
  ], { footer: [button('Cancel', closeDialog, 'btn-ghost'), save] });
}

export function createTicket(api: Api, act: Actions, messageId: number, text: string): void {
  const session = S.selected;
  if (!session) return;
  const [titleL, title] = textField('Ticket title', { value: text.split('\n')[0]?.slice(0, 80) ?? '' });
  const [goalL, goal] = areaField('Goal', { value: text, rows: 3 });
  const [pathsL, paths] = areaField('Allowed paths', { placeholder: 'crates/polyclave-os/\ndocs/notes.md', rows: 2, hint: 'One per line, relative to the repository. Required.' });
  const [forbidL, forbid] = areaField('Forbidden actions', { placeholder: 'push to main', rows: 2, hint: 'Optional, one per line.' });
  const [critL, crit] = areaField('Acceptance criteria', { placeholder: 'cargo test passes', rows: 2, hint: 'Optional, one per line.' });
  const [maxL, max] = textField('Run budget', { value: '4', type: 'text', hint: 'Maximum Worker runs for this ticket (1–12).' });
  max.inputMode = 'numeric';
  const err = errorLine();
  const save = button([icon('ticket'), 'Create ticket'], () => {
    const allowed = lines(paths.value);
    const budget = Number.parseInt(max.value, 10);
    if (!title.value.trim() || !goal.value.trim()) { err.textContent = 'A title and goal are required.'; return; }
    if (!allowed.length) { err.textContent = 'List at least one allowed path.'; return; }
    if (!Number.isInteger(budget) || budget < 1 || budget > 12) { err.textContent = 'The run budget must be a whole number from 1 to 12.'; return; }
    save.disabled = true;
    void api.createTicket(session, messageId, title.value.trim(), {
      goal: goal.value.trim(), allowed_paths: allowed, forbidden_actions: lines(forbid.value), acceptance_criteria: lines(crit.value),
    }, budget).then(async () => {
      closeDialog(); act.tell('Ticket created', 'info'); act.setPanel('Topology'); await act.refresh();
    }).catch(e => { save.disabled = false; err.textContent = e instanceof Error ? e.message : 'The ticket was not created'; });
  }, 'btn-primary');
  openDialog('Create ticket', [
    h('p', { text: 'A ticket records an explicit mandate from this message. Workers dispatched under it stay inside the allowed paths.' }),
    titleL, goalL, h('div', { class: 'grid-2' }, pathsL, forbidL), critL, maxL, err,
  ], { footer: [button('Cancel', closeDialog, 'btn-ghost'), save] });
}

export function dispatchWorker(api: Api, act: Actions, ticketId: string | null): void {
  const s = selectedSession();
  if (!s) { act.tell('Choose a project Hub first'); return; }
  let vendor: Vendor = 'codex';
  const vendorSeg = segmented('Worker vendor', ['codex', 'claude', 'agy', 'dsh'] as const, vendor, v => { vendor = v; }, { codex: 'Codex', claude: 'Claude', agy: 'Antigravity', dsh: 'DSH' });
  const [modelL, model] = textField('Model', { placeholder: 'Vendor default', hint: 'Leave empty to use the vendor default.', mono: true });
  const [branchL, branch] = textField('Base branch', { value: 'main', mono: true });
  const [instL, inst] = areaField('Instructions', { rows: 6, placeholder: 'Describe the change, the files involved and how to verify it.' });
  const err = errorLine();
  const go = button([icon('bolt'), 'Dispatch Worker'], () => {
    if (!inst.value.trim()) { err.textContent = 'Instructions are required.'; return; }
    if (!branch.value.trim()) { err.textContent = 'A base branch is required.'; return; }
    go.disabled = true;
    void api.dispatch({
      hub_session_id: s.id, request_id: crypto.randomUUID(), vendor, model: model.value.trim() || null, repo_cwd: s.repo_cwd,
      instructions: inst.value.trim(), base_branch: branch.value.trim(), ticket_id: ticketId,
    }).then(async run => {
      closeDialog(); act.tell('Worker ' + run.id + ' dispatched', 'info'); await act.refresh(); act.openRun(run.id, true);
    }).catch(e => { go.disabled = false; err.textContent = e instanceof Error ? e.message : 'The Worker was not dispatched'; });
  }, 'btn-primary');
  openDialog(ticketId ? 'Dispatch Worker · ' + ticketId : 'Dispatch Worker', [
    h('p', { text: 'The host prepares an isolated Attempt worktree from the base branch and verifies the result before you review it.' }),
    h('div', { class: 'field-label' }, h('span', { text: 'Vendor' }), vendorSeg), h('div', { class: 'grid-2' }, modelL, branchL), instL, err,
  ], { footer: [button('Cancel', closeDialog, 'btn-ghost'), go] });
}

export function startCouncilDialog(api: Api, act: Actions): void {
  const s = selectedSession();
  if (!s) { act.tell('Choose a project Hub first'); return; }
  let level: CouncilLevel = 'direction';
  let preference: PlanningPreference = 'balanced';
  const levelSeg = segmented('Council level', ['direction', 'detail'] as const, level, v => { level = v; }, { direction: 'Direction · user picks', detail: 'Detail · auto-selects if safe' });
  const prefSeg = segmented('Planning preference', ['thorough', 'balanced', 'conservative'] as const, preference, v => { preference = v; }, { thorough: 'Thorough', balanced: 'Balanced', conservative: 'Conservative' });
  const [promptL, prompt] = areaField('Proposal prompt', { rows: 5, placeholder: 'Describe the architectural question or implementation trade-offs for the blind council.' });
  const err = errorLine();
  const go = button([icon('bolt'), 'Start Council'], () => {
    const text = prompt.value.trim();
    if (!text) { err.textContent = 'A prompt is required.'; return; }
    go.disabled = true;
    void api.startCouncil({ hub_session_id: s.id, repo_cwd: s.repo_cwd, level, preference, prompt: text }).then(async council => {
      closeDialog();
      S.activeCouncil = council.id;
      S.reviewView = 'council';
      act.setPanel('Review');
      act.tell('Council ' + council.id + ' started', 'info');
      await act.refresh();
    }).catch(e => { go.disabled = false; err.textContent = e instanceof Error ? e.message : 'The council could not be started'; });
  }, 'btn-primary');
  openDialog('Start blind council', [
    h('p', { text: 'Enabled managers produce independent proposals without seeing each other. Vendor and model identities stay hidden until a decision is made.' }),
    h('div', { class: 'field-label' }, h('span', { text: 'Level' }), levelSeg),
    h('div', { class: 'field-label' }, h('span', { text: 'Preference' }), prefSeg),
    promptL, err,
  ], { footer: [button('Cancel', closeDialog, 'btn-ghost'), go] });
}

export function registerProject(api: Api, act: Actions): void {
  const [nameL, name] = textField('Project name', { placeholder: 'polyclave' });
  const [pathL, path] = textField('Local repository path', { placeholder: '~/Developer/project', mono: true, hint: 'The daemon resolves and checks the path before registering it.' });
  const err = errorLine();
  const go = button([icon('plus'), 'Register'], () => {
    if (!name.value.trim() || !path.value.trim()) { err.textContent = 'A name and a path are required.'; return; }
    go.disabled = true;
    void api.register(name.value.trim(), path.value.trim()).then(async s => {
      closeDialog(); await act.refresh(); act.selectSession(s.id);
    }).catch(e => { go.disabled = false; err.textContent = e instanceof Error ? e.message : 'The project was not registered'; });
  }, 'btn-primary');
  openDialog('Register project', [nameL, pathL, err], { footer: [button('Cancel', closeDialog, 'btn-ghost'), go] });
}

export function tokenDialog(act: Actions): void {
  const https = location.protocol === 'https:';
  // On the phone the pairing link is the only way in: the loopback capability
  // token is printed on the host's own machine and is useless here. A device
  // token can still be pasted if the link was consumed on another tab.
  const [tokL, tok] = textField(https ? 'Paired device token' : 'Local daemon token', {
    type: 'password', mono: true, autocomplete: 'off',
    hint: https ? 'Issued when the pairing link was opened. Leave empty to rescan a new link.' : 'Printed by polyclaved at start-up after #token=.',
  });
  const err = errorLine();
  const go = button(https ? 'Reconnect' : 'Connect', () => {
    const value = tok.value.trim();
    tok.value = '';
    if (!/^[0-9a-f]{32,64}$/i.test(value)) {
      err.textContent = https
        ? 'Open the pairing link again to get a new device token, or paste the token this device was given.'
        : 'Use the local daemon address (127.0.0.1) and its token.';
      return;
    }
    try {
      if (https) localStorage.setItem('pc-device', value); else sessionStorage.setItem('pc-session', value);
    } catch { act.tell('Storage is unavailable'); return; }
    location.reload();
  }, 'btn-primary');
  openDialog(https ? 'Reconnect this device' : 'Connect to the local daemon', [
    h('p', { text: https
      ? 'This device talks to Polyclave through the host\u2019s Tailscale HTTPS origin. Open a fresh pairing link from the host, or paste the device token it issued.'
      : 'Open the address polyclaved printed, or paste its token here. The token stays in this tab only.' }),
    tokL, err,
  ], { footer: [go] });
}

export function openPrGateDialog(api: Api, act: Actions, run: RunDto): void {
  const defaultReviewer: Vendor = run.vendor === 'claude' ? 'codex' : 'claude';
  let reviewerVendor: Vendor = defaultReviewer;
  const revSeg = segmented(
    'Heterogeneous reviewer',
    (['codex', 'claude', 'agy', 'dsh'] as const).filter(v => v !== run.vendor),
    defaultReviewer,
    v => { reviewerVendor = v; },
    { codex: 'Codex', claude: 'Claude', agy: 'Antigravity', dsh: 'DSH' },
  );
  const [slugL, slug] = textField('GitHub repository (owner/repo)', { placeholder: 'owner/repo', mono: true });
  const [branchL, branch] = textField('PR branch', { value: 'polyclave/' + run.id, mono: true });
  const [titleL, title] = textField('PR title', { value: (run.instructions.split('\n')[0] || 'Polyclave ' + run.id).slice(0, 80) });
  const [modelL, model] = textField('Reviewer model', { value: reviewerVendor === 'claude' ? 'claude-sonnet-4-6' : 'default', mono: true });
  const err = errorLine();
  const go = button([icon('review'), 'Open PR Gate'], () => {
    const repoSlug = slug.value.trim();
    if (!/^[^/\s]+\/[^/\s]+$/.test(repoSlug)) { err.textContent = 'Enter repository as owner/repo.'; return; }
    go.disabled = true;
    void api.runPrGate(run.id, {
      repo_slug: repoSlug,
      branch: branch.value.trim() || undefined,
      pr_title: title.value.trim() || undefined,
      reviewers: [{ vendor: reviewerVendor, model: model.value.trim() || 'default' }],
      hub_session_id: run.hub_session_id,
    }).then(async gate => {
      closeDialog();
      S.activePrGate = gate.id;
      S.reviewView = 'pr_gates';
      act.setPanel('Review');
      act.tell('PR Gate ' + gate.id + ' started', 'info');
      await act.refresh();
    }).catch(e => { go.disabled = false; err.textContent = e instanceof Error ? e.message : 'PR Gate could not be started'; });
  }, 'btn-primary');
  openDialog('Open PR Gate · ' + run.id, [
    h('p', { text: 'Pushes R1-verified commit ' + run.result_sha?.slice(0, 7) + ' and runs heterogeneous review before asking for your approval.' }),
    slugL, h('div', { class: 'grid-2' }, branchL, titleL),
    h('div', { class: 'field-label' }, h('span', { text: 'Reviewer vendor' }), revSeg),
    modelL, err,
  ], { footer: [button('Cancel', closeDialog, 'btn-ghost'), go] });
}
