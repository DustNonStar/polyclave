// Approval evidence: three separate judgments about one SHA, so the user knows
// exactly what an Accept covers.
//   1. Delivery (R1): the host's own check, read from Run classification.
//   2. Tests for this exact SHA: only what the host recorded for that SHA.
//   3. Request satisfied: always the user's judgment; the Worker's text is shown
//      as its claim, never as a check.
import type { RunDto, RunEvidenceDto } from '../generated/proto.js';
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

function tests(sha: string | null): [EvidenceRow, string] {
  // The host records no test result for any SHA (see PLAN-ISSUES.md), so this row
  // never shows a result and never borrows one from another SHA.
  const fact = sha ? 'No test result recorded for ' + short(sha) + '.' : 'No commit to test yet.';
  return [{
    key: 'tests', label: 'Tests for this exact SHA', state: 'No result recorded', tone: 'none', facts: [fact],
    source: 'Nothing records tests per SHA: submit_result carries no test summary, and the SCM observer does not store check results.',
  }, sha ? 'no test result for ' + short(sha) : 'no commit to test'];
}

function decision(r: RunDto, ev: RunEvidenceDto | null, sha: string | null): [EvidenceRow, string] {
  const raw = ev?.worker_claim?.trim();
  const claim = raw && raw.length > 600 ? raw.slice(0, 600) + '…' : raw;
  const facts = [
    claim ? 'Worker’s claim: “' + claim + '”' : 'The Worker sent no progress report for attempt ' + (ev?.attempt_no ?? r.current_attempt) + '.',
    'Not done or unverified items: the Worker has no field to list them; submit_result carries only the run, branch and SHA.',
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
  const [t, ts] = tests(sha);
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
