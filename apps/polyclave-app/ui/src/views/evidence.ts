// Approval evidence: three separate judgments about one SHA, so the user knows
// exactly what an Accept covers.
//   1. Delivery (R1): the host's own check, read from Run classification.
//   2. Tests for this exact SHA: only rows recorded for that SHA. Only a CI (or
//      future host-run) result for the Run's verified `result_sha` may turn green;
//      the Worker's claim is always labelled as a claim.
//   3. Request satisfied: always the user's judgment; the Worker's text is shown
//      as its claim, never as a check.
import type { RunDto, RunEvidenceDto, TestResultDto } from '../generated/proto.js';
import { h, short } from './elements.js';

export type EvidenceTone = 'done' | 'review' | 'fail' | 'none';
export interface EvidenceRow {
  key: 'delivery' | 'tests' | 'decision';
  label: string;
  state: string;
  tone: EvidenceTone;
  facts: string[];
  source: string;
}
export interface ApprovalEvidence { sha: string | null; summary: string; rows: EvidenceRow[] }

/** `classify.rs` sets this detail when a writer may still reach the old Attempt. */
const WRITER_DETAIL = 'a writer may still have access to the old Attempt';

function delivery(r: RunDto, ev: RunEvidenceDto | null, sha: string | null): [EvidenceRow, string] {
  const source = 'Host check at turn end (R1: signed receipt, authoritative mirror tip, base ancestry, Run-Id trailers)';
  const receipt = ev?.receipt_sha ?? null;
  const facts = [
    'Receipt: ' + (receipt ? short(receipt) : 'none submitted') + ' · mirror tip: ' + (r.observed_sha ? short(r.observed_sha) : 'no branch'),
  ];
  if (r.status === 'pending' || r.status === 'running') {
    return [{ key: 'delivery', label: 'Delivery verified (R1)', state: 'Not checked yet', tone: 'none', facts: ['The host checks delivery when the Worker’s turn ends.'], source }, 'commit not checked yet'];
  }
  if (r.status === 'done' && r.result_source === 'agent' && r.result_sha) {
    return [{
      key: 'delivery', label: 'Delivery verified (R1)', state: 'Verified', tone: 'done',
      facts: ['Receipt ' + short(receipt ?? r.result_sha) + ' equals the mirror tip ' + short(r.result_sha) + '; base ' + short(r.base_sha) + ' is an ancestor; every commit carries Run-Id ' + r.id + '.'],
      source,
    }, 'commit verified'];
  }
  if (r.status === 'done' && r.result_source === 'user') {
    return [{
      key: 'delivery', label: 'Delivery verified (R1)', state: 'Not verified', tone: 'fail',
      facts: ['R1 did not verify this delivery; you accepted ' + short(r.result_sha) + ' by hand.', ...facts], source,
    }, 'commit not verified'];
  }
  if (r.detail === WRITER_DETAIL) {
    return [{ key: 'delivery', label: 'Delivery verified (R1)', state: 'Needs review', tone: 'review', facts: ['Reason: ' + r.detail + '.', ...facts], source }, 'commit needs review'];
  }
  const reason = r.detail ?? (sha ? 'the host recorded no reason' : 'no commit was delivered');
  return [{ key: 'delivery', label: 'Delivery verified (R1)', state: 'Not verified', tone: 'fail', facts: ['Reason: ' + reason + '.', ...facts], source }, 'commit not verified'];
}

const STATUS_WORD: Record<string, string> = { passed: 'passed', failed: 'failed', pending: 'pending' };

function sourceName(row: TestResultDto): string {
  return row.source === 'ci' ? 'CI' : row.source === 'host_run' ? 'Host run' : 'Worker’s claim';
}

function clip(text: string, max: number): string {
  const trimmed = text.trim();
  return trimmed.length > max ? trimmed.slice(0, max) + '…' : trimmed;
}

function tests(r: RunDto, ev: RunEvidenceDto | null, sha: string | null): [EvidenceRow, string] {
  const label = 'Tests for this exact SHA';
  const target = ev?.test_sha ?? sha;
  // Only rows recorded for the SHA this card is about; an older commit's result
  // can never be borrowed, because the comparison is exact.
  const rows = (ev?.test_results ?? []).filter(t => target !== null && t.sha === target);
  const checks = rows.filter(t => t.source === 'ci' || t.source === 'host_run');
  const claims = rows.filter(t => t.source === 'worker_claim');
  const claimFacts = claims.map(c => 'Worker’s claim: “' + clip(c.summary, 300) + '” (' + (STATUS_WORD[c.status] ?? c.status) + ').');

  if (rows.length === 0) {
    const fact = target ? 'No test result recorded for ' + short(target) + '.' : 'No commit to test yet.';
    return [{
      key: 'tests', label, state: 'No result recorded', tone: 'none', facts: [fact],
      source: 'CI check runs are recorded per SHA when they finish; a test summary from submit_result is shown as the Worker’s claim, never as a host check.',
    }, target ? 'no test result for ' + short(target) : 'no commit to test'];
  }

  if (checks.length === 0) {
    // The Worker's own claim only: never green, whatever it says.
    return [{
      key: 'tests', label, state: 'Worker’s claim', tone: 'review',
      facts: [
        ...claimFacts,
        'Nothing host-side has checked ' + short(target ?? '') + ': no CI check run is recorded for this commit.',
      ],
      source: 'submit_result is the Worker speaking. Only CI (or a future host run) for this exact SHA can report a pass.',
    }, 'worker claim only for ' + short(target ?? '')];
  }

  const failed = checks.filter(t => t.status === 'failed');
  const pending = checks.filter(t => t.status === 'pending');
  // Green needs the host's own verification of exactly this SHA, not just a
  // matching `result_sha`: a hand accept (`result_source='user'`) sets that
  // field without an R1 receipt, so it can never turn this row green.
  const r1Verified = r.result_source === 'agent';
  const verified = r1Verified && target !== null && r.result_sha === target;
  const state = failed.length ? 'Failed' : pending.length ? 'Running' : verified ? 'Passed' : 'Reference only';
  const tone: EvidenceTone = failed.length ? 'fail' : pending.length ? 'review' : verified ? 'done' : 'review';
  const facts = checks.map(t =>
    sourceName(t) + ': ' + (t.name || 'check') + ' — ' + (STATUS_WORD[t.status] ?? t.status) + ' at ' + short(t.sha) + '.');
  if (!verified && !failed.length) {
    facts.push(target !== null && r.result_sha === target && !r1Verified
      ? 'You accepted ' + short(target) + ' by hand; R1 did not verify it, so its checks can never turn this row green.'
      : 'This is a reference, not a pass: the host has not verified ' + short(target ?? '') +
        (r.result_sha ? ', it verified ' + short(r.result_sha) : ', nothing is verified yet') + '.');
  }
  const claim0 = claims[0];
  if (claim0) {
    facts.push('The Worker also reports: “' + clip(claim0.summary, 300) + '” (' + (STATUS_WORD[claim0.status] ?? claim0.status) + '). A CI result wins over a claim.');
  }
  const tail = failed.length ? 'tests failed for ' + short(target ?? '')
    : pending.length ? 'tests still running for ' + short(target ?? '')
    : verified ? 'tests passed for ' + short(target ?? '')
    : 'tests recorded, not verified, for ' + short(target ?? '');
  return [{
    key: 'tests', label, state, tone, facts,
    source: 'Recorded by the host from GitHub check runs, keyed by the exact SHA; only the Run’s verified result can turn this green.',
  }, tail];
}

function decision(r: RunDto, ev: RunEvidenceDto | null, sha: string | null): [EvidenceRow, string] {
  const raw = ev?.worker_claim?.trim();
  const claim = raw && raw.length > 600 ? raw.slice(0, 600) + '…' : raw;
  const submitted = ev?.receipt_tests?.trim();
  const notDone = ev?.receipt_not_done?.trim();
  const facts = [
    claim ? 'Worker’s claim: “' + claim + '”' : 'The Worker sent no progress report for attempt ' + (ev?.attempt_no ?? r.current_attempt) + '.',
    submitted
      ? 'Tests, as the Worker submitted them' + (ev?.receipt_tests_status ? ' (' + (STATUS_WORD[ev.receipt_tests_status] ?? ev.receipt_tests_status) + ')' : '') + ': “' + clip(submitted, 400) + '”.'
      : 'The Worker submitted no test summary.',
    notDone
      ? 'Not done or unverified, per the Worker: “' + clip(notDone, 400) + '”.'
      : 'The Worker listed no unfinished or unverified items.',
  ];
  let tail: string;
  if (r.status === 'needs_review' || r.status === 'interrupted') tail = sha ? 'waiting for your decision' : 'nothing to accept';
  else if (r.status === 'pending' || r.status === 'running') tail = 'Worker still running';
  else if (r.status === 'done' && r.resolution === 'accept') { tail = 'you accepted ' + short(r.result_sha); facts.unshift('You accepted ' + short(r.result_sha) + '.'); }
  else if (r.status === 'failed') tail = 'closed as failed';
  else tail = 'not judged by you';
  return [{
    key: 'decision', label: 'Request satisfied', state: 'Your decision', tone: 'review', facts,
    source: 'Your judgment. Text from the Worker is its own claim; the host does not check it.',
  }, tail];
}

export function approvalEvidence(r: RunDto, ev: RunEvidenceDto | null | undefined): ApprovalEvidence {
  const sha = r.observed_sha ?? r.result_sha;
  const e = ev ?? null;
  const [d, ds] = delivery(r, e, sha);
  const [t, ts] = tests(r, e, sha);
  const [u, us] = decision(r, e, sha);
  const summary = ds.charAt(0).toUpperCase() + ds.slice(1) + ' · ' + ts + ' · ' + us;
  return { sha, summary, rows: [d, t, u] };
}

export function evidenceCard(ev: ApprovalEvidence): HTMLElement {
  const card = h('section', { class: 'card evidence', 'aria-label': 'Approval evidence' },
    h('p', { class: 'evidence-summary', 'data-evidence-summary': '', text: ev.summary }));
  const list = h('dl', { class: 'evidence-rows' });
  for (const row of ev.rows) {
    list.append(h('div', { class: 'evidence-row', 'data-evidence': row.key, 'data-tone': row.tone },
      h('dt', {},
        h('span', { class: 'evidence-label', text: row.label }),
        h('span', { class: 'pill-tag', 'data-tone': row.tone === 'none' ? undefined : row.tone, text: row.state })),
      h('dd', {},
        ...row.facts.map(f => h('p', { class: 'evidence-fact', text: f })),
        h('p', { class: 'evidence-source', text: row.source }))));
  }
  card.append(list);
  return card;
}
