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
const runs = [
  run('r2', { status: 'done', result_sha: verified, observed_sha: verified, result_source: 'agent', accepted_attempt_id: 7 }),
  run('r1', { status: 'needs_review', observed_sha: tip, detail: 'mirror tip differs from the receipt' }),
];
const commit = sha => ({ commits: sha.slice(0, 12) + ' fix(config): normalise Windows path separators', files: '14\t3\tcrates/config/src/load.rs\n22\t0\tcrates/config/tests/windows_paths.rs' });
const details = {
  r1: { run: runs[1], commit: commit(tip), evidence: { attempt_no: 2, receipt_sha: receipt, worker_claim: 'Changed the loader to normalise separators and added a regression test. Windows path tests not run on a Windows host.' } },
  r2: { run: runs[0], commit: commit(verified), evidence: { attempt_no: 2, receipt_sha: verified, worker_claim: null } },
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
