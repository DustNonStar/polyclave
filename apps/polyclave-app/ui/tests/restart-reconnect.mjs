// Offline browser check of a daemon restart: the token the page holds is rotated,
// every call now answers 401, and the page must re-authenticate once, drop the
// stale live deltas, and snapshot the same run again instead of showing them.
import assert from 'node:assert/strict';
import { resolve, sep } from 'node:path';
import { readFile } from 'node:fs/promises';
import { chromium } from 'playwright';

const root = resolve('dist');
const origin = 'http://127.0.0.1:4310';
const oldToken = 'a'.repeat(32);
const newToken = 'c'.repeat(32);
const run = {
  id: 'r1', ticket_id: 't1', hub_session_id: 'sfixture', worker_session_id: 'w1', request_id: 'q1',
  repo_cwd: 'fixture', vendor: 'codex', model: 'gpt-5', instructions: 'do it', context_refs_json: null,
  base_branch: 'main', base_sha: '1'.repeat(40), status: 'running', current_attempt: 1, max_attempts: 3,
  accepted_attempt_id: null, detail: null, result_sha: null, observed_sha: null, result_source: null,
  resolution: null, created_at: '2026-11-28T00:00:00Z', updated_at: '2026-11-28T00:00:00Z',
};
const session = { id: 'sfixture', name: 'Fixture workspace', role: 'hub', repo_cwd: 'fixture', profile_id: 'builtin-hub', provider_model: 'default' };
const measurements = [];
let token = oldToken;
let restarted = false;

async function handle(route) {
  const url = new URL(route.request().url());
  if (url.origin !== origin && !url.href.startsWith(origin)) { await route.abort(); return; }
  const path = url.pathname;
  const json = (body, status = 200) => route.fulfill({ status, contentType: 'application/json', body: JSON.stringify(body) });
  if (path.startsWith('/api/')) {
    if (route.request().headers()['authorization'] !== 'Bearer ' + token) return json({ message: 'rotated' }, 401);
    if (path === '/api/events') {
      // The first stream emits one live delta, then dies as the daemon restarts.
      if (!restarted) {
        restarted = true;
        return route.fulfill({
          status: 200, contentType: 'text/event-stream',
          body: 'event: agent\ndata: {"run_id":"r1","message":{"kind":"text_delta","text":"STALE-HALF-SENTENCE"}}\n\n',
        });
      }
      return route.fulfill({ status: 200, contentType: 'text/event-stream', body: ': ready\n\n' });
    }
    if (path === '/api/sessions') return json([session]);
    if (path === '/api/runs') return json([run]);
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
    token = oldToken; restarted = false;
    const page = await browser.newPage({ viewport: { width, height } });
    const errors = [];
    page.on('pageerror', e => errors.push(e.message));
    await page.route('**/*', handle);
    // The page picks the token up from its own `#token=` fragment, which is what
    // the daemon prints at start-up; nothing is injected behind its back.
    await page.goto(origin + '/?visit=1#token=' + oldToken);
    await page.waitForFunction(() => document.getElementById('connection')?.textContent === 'Local daemon connected', null, { timeout: 5000 });

    // The daemon restarts and rotates its token: every request is now 401.
    token = newToken;
    await page.waitForFunction(
      () => document.getElementById('connection')?.textContent !== 'Local daemon connected',
      null, { timeout: 8000 },
    );
    const text = await page.locator('#connection').textContent();
    assert.notEqual(text, 'Local daemon connected');
    // Nothing from the dead stream may survive as live output.
    assert.equal(await page.locator('body').evaluate(b => b.textContent.includes('STALE-HALF-SENTENCE')), false,
      'a delta from the old stream is not shown after the restart');
    const reconnect = page.getByRole('button', { name: /Reconnect|Connect/ });
    assert.ok(await reconnect.count() > 0, 'the page asks the person to reconnect');
    measurements.push({ step: 'rotated', width, connection: text });
    if (process.env.POLYCLAVE_HANDCHECK_DIR && width === 375) {
      await page.screenshot({ path: process.env.POLYCLAVE_HANDCHECK_DIR + '/item4-phone-rotated.png' });
    }

    // Reconnecting with the rotated token brings the same run back, without the
    // stale delta.
    assert.equal(token, newToken, 'the host now serves only the rotated token');
    await page.goto(origin + '/?visit=2#token=' + newToken);
    await page.waitForFunction(() => document.getElementById('connection')?.textContent === 'Local daemon connected', null, { timeout: 5000 });
    assert.equal(await page.locator('body').evaluate(b => b.textContent.includes('STALE-HALF-SENTENCE')), false,
      'the reconnected page starts from the snapshot');
    const overflow = await page.evaluate(() => ({ body: document.documentElement.scrollWidth, viewport: innerWidth }));
    assert.equal(overflow.body, width);
    measurements.push({ step: 'recovered', width, ...overflow });
    assert.deepEqual(errors, []);
    console.log('restart reconnect ' + width + 'x' + height + ' ok');
    await page.close();
  }
  if (process.env.POLYCLAVE_HANDCHECK_DIR) console.log(JSON.stringify(measurements, null, 1));
} finally {
  await browser.close();
}
