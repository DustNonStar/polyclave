// Offline browser check of the exact-SHA human approval: the gate stops for a
// person, the card shows that exact commit and its diff, and only a POST naming
// that SHA arms auto-merge. A stale SHA is refused and nothing is armed before.
import assert from 'node:assert/strict';
import { resolve, sep } from 'node:path';
import { readFile } from 'node:fs/promises';
import { chromium } from 'playwright';

const root = resolve('dist');
const shaB = 'b'.repeat(40);
const shaA = 'a'.repeat(40);
const shots = process.env.POLYCLAVE_HANDCHECK_DIR || null;
const session = { id: 'sfixture', name: 'Fixture workspace', role: 'hub', repo_cwd: 'fixture', profile_id: 'builtin-hub', provider_model: 'default' };
// Each viewport starts from a fresh gate: the previous run approved it.
const freshGate = () => ({
  id: 'g1', run_id: 'r1', ticket_id: 't1', repo_slug: 'org/repo', branch: 'feat/x',
  base_sha: 'c'.repeat(40), head_sha: shaB, pr_number: 42,
  pr_url: 'https://github.com/org/repo/pull/42', status: 'awaiting_human_approval',
  touched_paths: ['src/**'], scope_violations: [], revise_rounds: 0, auto_merge_armed: false,
  updated_at: '2026-11-28T00:00:00Z', reviews: [
    { id: 'v1', reviewer_vendor: 'claude', reviewer_model: 'sonnet', reviewed_sha: shaB, verdict: 'approve', comments: 'Clean diff', created_at: '2026-11-28T00:00:00Z' },
  ],
});
let gate = freshGate();
const measurements = [];
let state = { approvals: [], enabled: false, diffRequests: 0 };

async function handle(route) {
  const url = new URL(route.request().url());
  const path = url.pathname;
  const json = (body, status = 200) => route.fulfill({ status, contentType: 'application/json', body: JSON.stringify(body) });
  if (path.startsWith('/api/')) {
    if (route.request().headers()['authorization'] !== 'Bearer ' + 'f'.repeat(32)) return json({}, 401);
    if (path === '/api/events') return route.fulfill({ status: 200, contentType: 'text/event-stream', body: ': ready\n\n' });
    if (path === '/api/pr-gates') return json([gate]);
    if (path === '/api/pr-gates/g1/diff') {
      state.diffRequests += 1;
      return json({
        gate_id: 'g1', base_sha: gate.base_sha, head_sha: shaB,
        text: 'Commits (1):\n' + shaB + ' change\n src/lib.rs | 2 +-\n\n--- a/src/lib.rs\n+++ b/src/lib.rs\n+pub fn v() -> i32 { 2 }\n',
        bytes: 128, truncated: false,
      });
    }
    if (path === '/api/pr-gates/g1/approve' && route.request().method() === 'POST') {
      const body = JSON.parse(route.request().postData() ?? '{}');
      state.approvals.push(body.expected_sha);
      // Only the SHA the gate carries may be approved.
      if (body.expected_sha !== gate.head_sha) return json({ message: 'the gate moved' }, 409);
      state.enabled = true;
      Object.assign(gate, { status: 'auto_merge_armed', auto_merge_armed: true });
      return json(gate);
    }
    if (path === '/api/sessions') return json([session]);
    if (path === '/api/runs/r1') {
      return json({
        run: {
          id: 'r1', ticket_id: 't1', hub_session_id: 'sfixture', worker_session_id: 'w1', request_id: 'q1',
          repo_cwd: 'fixture', vendor: 'codex', model: 'gpt-5', instructions: 'do it', context_refs_json: null,
          base_branch: 'main', base_sha: gate.base_sha, status: 'done', current_attempt: 1, max_attempts: 5,
          accepted_attempt_id: 1, detail: null, result_sha: shaB, observed_sha: shaB, result_source: 'agent',
          resolution: null, created_at: '2026-11-28T00:00:00Z', updated_at: '2026-11-28T00:00:00Z',
        },
        commit: null,
        evidence: {
          attempt_no: 1,
          receipt_sha: shaB,
          worker_claim: 'implemented v2',
          test_sha: shaB,
          receipt_tests: 'cargo test',
          receipt_tests_status: 'passed',
          receipt_not_done: 'Windows CI pending',
          test_results: [
            { id: 1, run_id: 'r1', sha: shaB, source: 'ci', name: 'ci/unit', check_id: '101', status: 'passed', summary: 'ci/unit: success', created_at: '2026-11-28T00:00:00Z' },
          ],
        },
      });
    }
    if (path === '/api/pause') return json({ paused: false, reason: null, updated_at: '2026-11-28T00:00:00Z' });
    return json([]);
  }
  const file = path === '/' ? 'index.html' : path.slice(1);
  const exact = resolve(root, file);
  if (!exact.startsWith(root + sep)) { await route.fulfill({ status: 403, body: '' }); return; }
  const type = file.endsWith('.css') ? 'text/css' : file.endsWith('.js') ? 'text/javascript' : 'text/html';
  await route.fulfill({ body: await readFile(exact), contentType: type });
}

const { POLYCLAVE_CHROMIUM, POLYCLAVE_BROWSER_CHANNEL } = process.env;
const browser = await chromium.launch(POLYCLAVE_CHROMIUM ? { executablePath: POLYCLAVE_CHROMIUM } : POLYCLAVE_BROWSER_CHANNEL === 'chrome' ? { channel: 'chrome' } : {});
try {
  for (const [width, height] of [[1440, 900], [375, 812]]) {
    gate = freshGate();
    state = { approvals: [], enabled: false, diffRequests: 0 };
    const page = await browser.newPage({ viewport: { width, height } });
    const errors = [];
    page.on('pageerror', e => errors.push(e.message));
    await page.route('**/*', handle);
    await page.addInitScript(() => { try { sessionStorage.setItem('pc-session', 'f'.repeat(32)); } catch { /* ignore */ } });
    await page.goto('http://127.0.0.1:4310/?visit=1');
    await page.waitForFunction(() => document.getElementById('connection')?.textContent === 'Local daemon connected', null, { timeout: 5000 });
    // Needs you count must be > 0 and match attentionPrGates() length (1).
    await page.waitForFunction(() => document.getElementById('needs-count')?.textContent === '1', null, { timeout: 5000 });
    if (width === 375) {
      await page.getByRole('button', { name: 'Inspect', exact: true }).click();
      await page.locator('#tab-review').click();
      await page.locator('[data-focus-key="rev-sub-pr_gates"]').click({ timeout: 5000 });
    } else {
      await page.locator('#needs-you').click();
    }
    await page.locator('.pr-gate-card').waitFor({ state: 'visible', timeout: 5000 });

    // Nothing is armed, and the card names the exact commit it wants approved.
    assert.equal(state.enabled, false);
    const card = page.locator('.pr-gate-card');
    assert.match(await card.textContent(), /awaiting human approval/);
    assert.match(await card.textContent(), new RegExp('bbbbbbb'), 'the exact SHA is on the card');
    assert.equal(await page.locator('[data-pr-link="true"]').getAttribute('href'), 'https://github.com/org/repo/pull/42');
    assert.match(await card.textContent(), /ci\/unit/);
    assert.match(await card.textContent(), /Windows CI pending/);
    assert.equal(await page.locator('[data-focus-key="run-pr-gate"]').count(), 0, 'Run PR Gate button hidden while awaiting approval');
    const approve = page.locator('[data-focus-key="approve-pr-gate"]');
    assert.equal((await approve.textContent()).trim(), 'Approve bbbbbbb');

    // The diff is only fetched when asked for, then shown with its range.
    assert.equal(state.diffRequests, 0);
    await page.locator('[data-focus-key="show-diff"]').click();
    await page.locator('[data-diff="text"]').waitFor({ state: 'visible' });
    assert.equal(state.diffRequests, 1);
    assert.match(await page.locator('[data-diff="text"]').textContent(), /src\/lib\.rs/);
    const overflow = await page.evaluate(() => ({ body: document.documentElement.scrollWidth, viewport: innerWidth }));
    assert.equal(overflow.body, width, 'no horizontal overflow with the diff open');
    measurements.push({ step: 'awaiting', width, ...overflow, diffRequests: state.diffRequests });
    if (shots && width === 375) { await page.locator('.pr-gate-card').screenshot({ path: shots + '/item3-phone-awaiting.png' }); }
    if (shots && width === 1440) { await page.locator('.pr-gate-card').screenshot({ path: shots + '/item3-desktop-awaiting.png' }); }

    // Approving this SHA arms it, and the POST carried exactly that SHA.
    await approve.click();
    await page.waitForFunction(() => document.querySelector('.pr-gate-card')?.textContent?.includes('auto merge armed'));
    assert.equal(await page.locator('[data-focus-key="run-pr-gate"]').count(), 0, 'Run PR Gate button hidden while armed');
    assert.deepEqual(state.approvals, [shaB]);
    assert.equal(state.enabled, true);
    measurements.push({ step: 'approved', width, approvals: state.approvals });
    if (shots && width === 375) { await page.locator('.pr-gate-card').screenshot({ path: shots + '/item3-phone-approved.png' }); }

    assert.deepEqual(errors, []);
    console.log('pr gate approval ' + width + 'x' + height + ' ok');
    await page.close();
  }
  if (shots) console.log(JSON.stringify(measurements, null, 1));
} finally {
  await browser.close();
}
