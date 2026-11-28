// Offline browser check of the Run approval card: delivery (R1), tests for the exact SHA
// and the user's decision are separate rows, and the summary never claims tests passed
// when the host recorded none. Set POLYCLAVE_HANDCHECK_DIR to also save screenshots.
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { mkdir, readFile } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import { chromium } from 'playwright';

const root = resolve('dist');
const capability = 'b'.repeat(32);
const shots = process.env.POLYCLAVE_HANDCHECK_DIR || null;
const session = { id: 'sfixture', name: 'Fixture workspace', role: 'hub', repo_cwd: 'fixture', profile_id: 'builtin-hub', provider_model: 'default' };
const base = '1111111111111111111111111111111111111111';
const receipt = 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa';
const tip = 'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb';
const verified = 'cccccccccccccccccccccccccccccccccccccccc';
const run = (id, extra) => ({
  id, ticket_id: 't1', hub_session_id: 'sfixture', worker_session_id: 'w-' + id, request_id: 'q-' + id, repo_cwd: 'fixture',
  vendor: 'codex', model: 'gpt-5.5', instructions: 'Normalise Windows paths in the config loader', context_refs_json: null,
  base_branch: 'main', base_sha: base, current_attempt: 2, max_attempts: 3, accepted_attempt_id: null, detail: null,
  result_sha: null, observed_sha: null, result_source: null, resolution: null,
  created_at: '2026-11-27T00:00:00Z', updated_at: '2026-11-27T00:05:00Z', ...extra,
});
const recorded = 'd'.repeat(40);
const conflicted = 'e'.repeat(40);
const claimOnly = 'f'.repeat(40);
const olderSha = 'a'.repeat(40);
const handAccepted = '9'.repeat(40);
const partial = '7'.repeat(40);
const runs = [
  run('r2', { status: 'done', result_sha: verified, observed_sha: verified, result_source: 'agent', accepted_attempt_id: 7 }),
  run('r1', { status: 'needs_review', observed_sha: tip, detail: 'mirror tip differs from the receipt' }),
  run('r3', { status: 'done', result_sha: recorded, observed_sha: recorded, result_source: 'agent', accepted_attempt_id: 8 }),
  run('r4', { status: 'done', result_sha: conflicted, observed_sha: conflicted, result_source: 'agent', accepted_attempt_id: 9 }),
  run('r5', { status: 'needs_review', observed_sha: claimOnly, detail: 'mirror tip differs from the receipt' }),
  // Accepted by a person by hand: `result_sha` is set but R1 never receipted it.
  run('r6', { status: 'done', result_sha: handAccepted, observed_sha: handAccepted, result_source: 'user', resolution: 'accept' }),
  // A partial check-run list: the host's marker keeps the row non-green.
  run('r7', { status: 'done', result_sha: partial, observed_sha: partial, result_source: 'agent', accepted_attempt_id: 10 }),
];
const commit = sha => ({ commits: sha.slice(0, 12) + ' fix(config): normalise Windows path separators', files: '14\t3\tcrates/config/src/load.rs\n22\t0\tcrates/config/tests/windows_paths.rs' });
const result = (sha, source, status, name, summary) => ({ sha, source, status, name, summary, recorded_at: '2026-11-27T00:06:00Z' });
const details = {
  r1: { run: runs[1], commit: commit(tip), evidence: { attempt_no: 2, receipt_sha: receipt, worker_claim: 'Changed the loader to normalise separators and added a regression test. Windows path tests not run on a Windows host.', test_sha: tip, receipt_tests: null, receipt_tests_status: null, receipt_not_done: null, test_results: [] } },
  r2: { run: runs[0], commit: commit(verified), evidence: { attempt_no: 2, receipt_sha: verified, worker_claim: null, test_sha: verified, receipt_tests: null, receipt_tests_status: null, receipt_not_done: null, test_results: [] } },
  // CI for the verified SHA, the Worker's claim for the same SHA, and a result
  // recorded for an older commit that must never be shown here.
  r3: {
    run: runs[2], commit: commit(recorded),
    evidence: {
      attempt_no: 2, receipt_sha: recorded, worker_claim: 'Ran the suite locally before submitting.',
      test_sha: recorded,
      receipt_tests: 'cargo test: 42 passed', receipt_tests_status: 'passed',
      receipt_not_done: 'Windows path tests not run',
      test_results: [
        result(olderSha, 'ci', 'failed', 'ci/old', 'ci/old: failure'),
        result(recorded, 'ci', 'passed', 'ci/rust-test', 'ci/rust-test: success'),
        result(recorded, 'worker_claim', 'passed', '', 'cargo test: 42 passed'),
      ],
    },
  },
  // CI failed while the Worker claimed a pass: CI wins and the claim is still shown.
  r4: {
    run: runs[3], commit: commit(conflicted),
    evidence: {
      attempt_no: 2, receipt_sha: conflicted, worker_claim: null,
      test_sha: conflicted,
      receipt_tests: 'all green locally', receipt_tests_status: 'passed', receipt_not_done: null,
      test_results: [
        result(conflicted, 'ci', 'failed', 'ci/rust-test', 'ci/rust-test: failure'),
        result(conflicted, 'worker_claim', 'passed', '', 'all green locally'),
      ],
    },
  },
  // Only the Worker's own claim: never green.
  r5: {
    run: runs[4], commit: commit(claimOnly),
    evidence: {
      attempt_no: 2, receipt_sha: claimOnly, worker_claim: null,
      test_sha: claimOnly,
      receipt_tests: 'my tests pass', receipt_tests_status: 'passed', receipt_not_done: null,
      test_results: [result(claimOnly, 'worker_claim', 'passed', '', 'my tests pass')],
    },
  },
  // The host could only read part of the check-run list, so it recorded its
  // pending marker; nothing may turn green on an incomplete list.
  r7: {
    run: runs[6], commit: commit(partial),
    evidence: {
      attempt_no: 2, receipt_sha: partial, worker_claim: null,
      test_sha: partial,
      receipt_tests: null, receipt_tests_status: null, receipt_not_done: null,
      test_results: [
        result(partial, 'ci', 'passed', 'ci/unit', 'ci/unit: success'),
        result(partial, 'ci', 'pending', 'polyclave/check-runs', 'polyclave/check-runs: in_progress'),
      ],
    },
  },
  // CI passed for this SHA, but a hand accept is not an R1 receipt: the row may
  // not turn green on the strength of `result_sha` alone.
  r6: {
    run: runs[5], commit: commit(handAccepted),
    evidence: {
      attempt_no: 2, receipt_sha: null, worker_claim: null,
      test_sha: handAccepted,
      receipt_tests: null, receipt_tests_status: null, receipt_not_done: null,
      test_results: [result(handAccepted, 'ci', 'passed', 'ci/rust-test', 'ci/rust-test: success')],
    },
  },
};
const clients = new Set();
const server = createServer(async (req, res) => {
  const path = new URL(req.url ?? '/', 'http://127.0.0.1').pathname;
  if (path.startsWith('/api/')) {
    if (req.headers.authorization !== 'Bearer ' + capability) { res.writeHead(401).end(); return; }
    if (path === '/api/events') {
      res.writeHead(200, { 'content-type': 'text/event-stream' }); res.write(': ready\n\n');
      clients.add(res); req.on('close', () => clients.delete(res)); return;
    }
    for await (const _ of req) { /* drain */ }
    res.setHeader('content-type', 'application/json');
    const m = path.match(/^\/api\/runs\/([^/]+)$/);
    const data = m ? details[decodeURIComponent(m[1])] : path === '/api/sessions' ? [session] : path === '/api/runs' ? runs
      : path === '/api/profiles' ? [{ id: 'builtin-hub', kind: 'codex_cli', display_name: 'Codex CLI', models: ['default'] }]
      : path === '/api/pause' ? { paused: false, reason: null, updated_at: '2026-11-27T00:00:00Z' } : [];
    if (data === undefined) { res.writeHead(404).end('{}'); return; }
    res.end(JSON.stringify(data)); return;
  }
  try {
    const file = path === '/' ? 'index.html' : path.slice(1);
    const exact = resolve(root, file);
    if (!exact.startsWith(root + '/') && !exact.startsWith(root + '\\')) { res.writeHead(403).end(); return; }
    res.setHeader('content-type', file.endsWith('.css') ? 'text/css' : file.endsWith('.js') ? 'text/javascript' : 'text/html');
    res.end(await readFile(exact));
  } catch { res.writeHead(404).end(); }
});
await new Promise(r => server.listen(0, '127.0.0.1', r));
const origin = 'http://127.0.0.1:' + server.address().port;
const PASS_CLAIM = /(tests?|checks?|ci)\b[^·]*\b(pass(ed|ing)?|green|succeeded|ok)\b|✓|✔/i;
const measurements = [];

async function card(page) {
  return page.locator('.evidence').evaluate(c => ({
    summary: c.querySelector('[data-evidence-summary]').textContent,
    rows: [...c.querySelectorAll('.evidence-row')].map(r => ({
      key: r.dataset.evidence, state: r.querySelector('.pill-tag').textContent,
      label: r.querySelector('.evidence-label').textContent, text: r.querySelector('dd').textContent,
    })),
  }));
}
async function overflow(page) {
  return page.evaluate(() => {
    const scope = [...document.querySelectorAll('#inspect *')];
    const wide = scope.filter(e => {
      const s = getComputedStyle(e);
      return e.clientWidth > 0 && e.scrollWidth > e.clientWidth + 1 && s.overflowX === 'visible';
    }).map(e => e.className || e.tagName);
    const ev = document.querySelector('.evidence').getBoundingClientRect();
    return { wide, body: document.documentElement.scrollWidth, viewport: innerWidth, card: [Math.round(ev.left), Math.round(ev.right)] };
  });
}

const { POLYCLAVE_CHROMIUM, POLYCLAVE_BROWSER_CHANNEL } = process.env;
const browser = await chromium.launch(POLYCLAVE_CHROMIUM ? { executablePath: POLYCLAVE_CHROMIUM } : POLYCLAVE_BROWSER_CHANNEL === 'chrome' ? { channel: 'chrome' } : {});
try {
  if (shots) await mkdir(shots, { recursive: true });
  for (const [width, height] of [[1440, 900], [375, 812]]) {
    for (const theme of ['dark', 'light']) {
      const page = await browser.newPage({ viewport: { width, height } });
      const errors = []; const outside = [];
      page.on('pageerror', e => errors.push(e.message));
      await page.route('**/*', async route => {
        if (!route.request().url().startsWith(origin + '/')) { outside.push(route.request().url()); await route.abort(); return; }
        await route.continue();
      });
      await page.addInitScript(t => { try { localStorage.setItem('pc-theme', t); } catch { /* ignore */ } }, theme);
      await page.emulateMedia({ reducedMotion: 'reduce', colorScheme: theme });
      await page.goto(origin + '/#token=' + capability);
      await page.waitForFunction(() => document.getElementById('connection')?.textContent === 'Local daemon connected', null, { timeout: 5000 });
      assert.equal(await page.locator('html').getAttribute('data-theme'), theme);
      if (width === 375) await page.getByRole('button', { name: 'Workspace', exact: true }).click();
      await page.locator('#needs-you').click();
      await page.locator('.evidence').waitFor({ state: 'visible' });
      await page.waitForFunction(() => document.querySelector('[data-evidence="decision"]')?.textContent.includes('Worker’s claim'));

      // r1: receipt differs from the mirror tip, no test record.
      const r1 = await card(page);
      assert.equal(r1.summary, 'Commit not verified · no test result for bbbbbbb · waiting for your decision');
      assert.doesNotMatch(r1.summary, PASS_CLAIM);
      assert.deepEqual(r1.rows.map(r => [r.key, r.label, r.state]), [
        ['delivery', 'Delivery verified (R1)', 'Not verified'],
        ['tests', 'Tests for this exact SHA', 'No result recorded'],
        ['decision', 'Request satisfied', 'Your decision'],
      ]);
      assert.ok(r1.rows[0].text.includes('Reason: mirror tip differs from the receipt.'));
      assert.ok(r1.rows[0].text.includes('Receipt: aaaaaaa · mirror tip: bbbbbbb'));
      assert.ok(r1.rows[1].text.includes('No test result recorded for bbbbbbb.'));
      assert.doesNotMatch(r1.rows[1].text, PASS_CLAIM);
      assert.ok(r1.rows[2].text.includes('Worker’s claim: “Changed the loader'));
      assert.equal((await page.locator('[data-focus-key="accept"]').textContent()).trim(), 'Accept bbbbbbb');
      const o1 = await overflow(page);
      assert.deepEqual(o1.wide, []); assert.equal(o1.body, width);
      assert.ok(o1.card[0] >= 0 && o1.card[1] <= width, 'evidence card inside the viewport');
      let small = [];
      if (width === 375) {
        small = await page.evaluate(() => [...document.querySelectorAll('#inspect button, #inspect select')].filter(b => {
          const r = b.getBoundingClientRect(); return r.width > 0 && r.height > 0 && r.height < 44;
        }).map(b => b.textContent.trim().slice(0, 24) || b.className));
        assert.deepEqual(small, []);
      }
      measurements.push({ run: 'r1', width, theme, summary: r1.summary, ...o1, under44: small });
      if (shots) {
        const name = 'r1-' + (width === 375 ? 'phone' : 'desktop') + '-' + theme;
        await page.screenshot({ path: join(shots, name + '.png') });
        await page.locator('.evidence').screenshot({ path: join(shots, name + '-card.png') });
        if (width === 375) { await page.locator('.decision').scrollIntoViewIfNeeded(); await page.screenshot({ path: join(shots, name + '-accept.png') }); }
      }

      // r2: R1 verified, still no test record; tests must not turn green by default.
      await page.locator('select.run-picker').selectOption('r2');
      await page.waitForFunction(() => document.querySelector('[data-evidence-summary]')?.textContent.startsWith('Commit verified'));
      const r2 = await card(page);
      assert.equal(r2.summary, 'Commit verified · no test result for ccccccc · not judged by you');
      assert.doesNotMatch(r2.summary, PASS_CLAIM);
      assert.deepEqual(r2.rows.map(r => r.state), ['Verified', 'No result recorded', 'Your decision']);
      assert.ok(r2.rows[1].text.includes('No test result recorded for ccccccc.'));
      assert.equal(await page.locator('[data-evidence="tests"] .pill-tag').getAttribute('data-tone'), null);
      const o2 = await overflow(page);
      assert.deepEqual(o2.wide, []); assert.equal(o2.body, width);
      measurements.push({ run: 'r2', width, theme, summary: r2.summary, ...o2 });
      if (shots && theme === 'dark') await page.locator('.evidence').screenshot({ path: join(shots, 'r2-' + (width === 375 ? 'phone' : 'desktop') + '-dark-card.png') });

      // r3: CI passed for the verified SHA, the Worker's claim is shown beside it,
      // and the older commit's failure is not borrowed.
      await page.locator('select.run-picker').selectOption('r3');
      await page.waitForFunction(() => document.querySelector('[data-evidence-summary]')?.textContent.includes('tests passed for ddddddd'));
      const r3 = await card(page);
      assert.equal(r3.summary, 'Commit verified · tests passed for ddddddd · not judged by you');
      assert.deepEqual(r3.rows.map(r => r.state), ['Verified', 'Passed', 'Your decision']);
      assert.equal(await page.locator('[data-evidence="tests"] .pill-tag').getAttribute('data-tone'), 'done');
      assert.ok(r3.rows[1].text.includes('CI: ci/rust-test — passed at ddddddd.'));
      assert.ok(!r3.rows[1].text.includes('ci/old'), 'an older SHA result is never shown');
      assert.ok(r3.rows[1].text.includes('A CI result wins over a claim'));
      assert.ok(r3.rows[2].text.includes('Tests, as the Worker submitted them (passed): “cargo test: 42 passed”'));
      assert.ok(r3.rows[2].text.includes('Not done or unverified, per the Worker: “Windows path tests not run”'));
      const o3 = await overflow(page);
      assert.deepEqual(o3.wide, []); assert.equal(o3.body, width);
      measurements.push({ run: 'r3', width, theme, summary: r3.summary,
        tests: { state: r3.rows[1].state, tone: await page.locator('[data-evidence="tests"] .pill-tag').getAttribute('data-tone') }, ...o3 });
      if (shots) {
        const name = 'r3-' + (width === 375 ? 'phone' : 'desktop') + '-' + theme;
        await page.screenshot({ path: join(shots, name + '.png') });
        await page.locator('.evidence').screenshot({ path: join(shots, name + '-card.png') });
      }

      // r4: a failed CI check outranks the Worker's claim, which is still shown.
      await page.locator('select.run-picker').selectOption('r4');
      await page.waitForFunction(() => document.querySelector('[data-evidence-summary]')?.textContent.includes('tests failed for eeeeeee'));
      const r4 = await card(page);
      assert.equal(r4.summary, 'Commit verified · tests failed for eeeeeee · not judged by you');
      assert.equal(r4.rows[1].state, 'Failed');
      assert.equal(await page.locator('[data-evidence="tests"] .pill-tag').getAttribute('data-tone'), 'fail');
      assert.ok(r4.rows[1].text.includes('CI: ci/rust-test — failed at eeeeeee.'));
      assert.ok(r4.rows[1].text.includes('The Worker also reports: “all green locally” (passed). A CI result wins over a claim.'));
      const o4 = await overflow(page);
      assert.deepEqual(o4.wide, []); assert.equal(o4.body, width);
      measurements.push({ run: 'r4', width, theme, summary: r4.summary, tests: { state: r4.rows[1].state }, ...o4 });
      if (shots && theme === 'dark') await page.locator('.evidence').screenshot({ path: join(shots, 'r4-' + (width === 375 ? 'phone' : 'desktop') + '-dark-card.png') });

      // r5: only the Worker's claim is recorded, so the row stays orange and the
      // summary never says tests passed.
      await page.locator('select.run-picker').selectOption('r5');
      await page.waitForFunction(() => document.querySelector('[data-evidence-summary]')?.textContent.includes('worker claim only for fffffff'));
      const r5 = await card(page);
      assert.equal(r5.summary, 'Commit not verified · worker claim only for fffffff · waiting for your decision');
      assert.doesNotMatch(r5.summary, PASS_CLAIM);
      assert.equal(r5.rows[1].state, 'Worker’s claim');
      assert.equal(await page.locator('[data-evidence="tests"] .pill-tag').getAttribute('data-tone'), 'review');
      assert.ok(r5.rows[1].text.includes('no CI check run is recorded for this commit'));
      // The row may quote a claim that says "my tests pass"; what matters is that
      // the card itself stays neutral and never turns green.
      assert.notEqual(await page.locator('[data-evidence="tests"] .pill-tag').getAttribute('data-tone'), 'done');
      const o5 = await overflow(page);
      assert.deepEqual(o5.wide, []); assert.equal(o5.body, width);
      measurements.push({ run: 'r5', width, theme, summary: r5.summary, tests: { state: r5.rows[1].state }, ...o5 });
      if (shots && theme === 'dark') await page.locator('.evidence').screenshot({ path: join(shots, 'r5-' + (width === 375 ? 'phone' : 'desktop') + '-dark-card.png') });

      // r6: a hand-accepted commit. Its CI passed, yet without an R1 receipt the
      // tests row stays a reference and never turns green.
      await page.locator('select.run-picker').selectOption('r6');
      await page.waitForFunction(() => document.querySelector('[data-evidence-summary]')?.textContent.includes('tests recorded, not verified'));
      const r6 = await card(page);
      assert.equal(r6.summary, 'Commit not verified · tests recorded, not verified, for 9999999 · you accepted 9999999');
      assert.doesNotMatch(r6.summary, /\b(passed|green)\b/i);
      assert.deepEqual(r6.rows.map(r => r.state), ['Not verified', 'Reference only', 'Your decision']);
      assert.equal(await page.locator('[data-evidence="tests"] .pill-tag').getAttribute('data-tone'), 'review');
      assert.ok(r6.rows[0].text.includes('you accepted 9999999 by hand'));
      assert.ok(r6.rows[1].text.includes('You accepted 9999999 by hand; R1 did not verify it, so its checks can never turn this row green.'));
      const o6 = await overflow(page);
      assert.deepEqual(o6.wide, []); assert.equal(o6.body, width);
      measurements.push({ run: 'r6', width, theme, summary: r6.summary, tests: { state: r6.rows[1].state }, ...o6 });
      if (shots && theme === 'dark') await page.locator('.evidence').screenshot({ path: join(shots, 'r6-' + (width === 375 ? 'phone' : 'desktop') + '-dark-card.png') });

      // r7: a partial check-run list never turns the tests row green.
      await page.locator('select.run-picker').selectOption('r7');
      await page.waitForFunction(() => document.querySelector('[data-evidence-summary]')?.textContent.includes('tests still running'));
      const r7 = await card(page);
      assert.equal(r7.summary, 'Commit verified · tests still running for 7777777 · not judged by you');
      assert.equal(r7.rows[1].state, 'Running');
      assert.equal(await page.locator('[data-evidence="tests"] .pill-tag').getAttribute('data-tone'), 'review');
      assert.ok(r7.rows[1].text.includes('could not read every check run for this commit'));
      assert.ok(r7.rows[1].text.includes('CI: ci/unit — passed at 7777777.'));
      const o7 = await overflow(page);
      assert.deepEqual(o7.wide, []); assert.equal(o7.body, width);
      measurements.push({ run: 'r7', width, theme, summary: r7.summary, tests: { state: r7.rows[1].state }, ...o7 });
      if (shots && theme === 'dark') await page.locator('.evidence').screenshot({ path: join(shots, 'r7-' + (width === 375 ? 'phone' : 'desktop') + '-dark-card.png') });

      assert.deepEqual(errors, []); assert.deepEqual(outside, []);
      console.log('approval card ' + width + 'x' + height + ' ' + theme + ' ok');
      await page.close();
    }
  }
  if (shots) console.log(JSON.stringify(measurements, null, 1));
} finally {
  await browser.close();
  for (const c of clients) c.end();
  await new Promise(r => server.close(r));
}
