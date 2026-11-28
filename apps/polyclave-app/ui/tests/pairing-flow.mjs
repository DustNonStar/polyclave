// Offline browser check of the phone pairing flow: the QR link opens at the
// configured Tailscale HTTPS origin, the page exchanges the one-time secret for a
// device token, strips the secret from the address bar, then uses that token for
// the API and the event stream. A replayed link is refused and the device keeps
// the token it already has. Every request is served by the test, so nothing
// leaves the machine and no certificate is needed.
import assert from 'node:assert/strict';
import { resolve, sep } from 'node:path';
import { readFile } from 'node:fs/promises';
import { chromium } from 'playwright';

const root = resolve('dist');
const origin = 'https://polyclave.test';
const firstToken = 'a'.repeat(32);
const secondToken = 'b'.repeat(32);
const shots = process.env.POLYCLAVE_HANDCHECK_DIR || null;
const session = { id: 'sfixture', name: 'Fixture workspace', role: 'hub', repo_cwd: 'fixture', profile_id: 'builtin-hub', provider_model: 'default' };
const measurements = [];

// Each viewport starts from a fresh host: a spent ticket from the previous run
// must not leak into the next one.
const freshState = () => ({
  secrets: new Map([['pair-one', firstToken], ['pair-two', secondToken]]),
  spent: new Set(),
  exchanges: [],
  tokensSeen: [],
  sse: 0,
  revoked: false,
});
let state = freshState();

async function handle(route) {
  const url = new URL(route.request().url());
  if (url.origin !== origin) { await route.abort(); return; }
  const path = url.pathname;
  const json = (body, status = 200) => route.fulfill({ status, contentType: 'application/json', body: JSON.stringify(body) });
  if (path === '/api/pairing/exchange') {
    const body = JSON.parse(route.request().postData() ?? '{}');
    state.exchanges.push(body.secret);
    const token = state.secrets.get(body.secret);
    if (!token) return json({ message: 'unknown ticket' }, 404);
    if (state.spent.has(body.secret)) return json({ message: 'ticket already used' }, 409);
    state.spent.add(body.secret);
    return json({ token, device: { id: 'dev-1', label: 'Phone', client_kind: 'ios', peer_identity: 'dust@tailnet' } });
  }
  if (path.startsWith('/api/')) {
    const auth = route.request().headers()['authorization'] ?? '';
    state.tokensSeen.push(auth);
    const known = [firstToken, secondToken].some(t => auth === 'Bearer ' + t);
    if (state.revoked || !known) return json({ message: 'not authorized' }, 401);
    if (path === '/api/events') {
      state.sse += 1;
      return route.fulfill({ status: 200, contentType: 'text/event-stream', body: ': ready\n\n' });
    }
    if (path === '/api/sessions') return json([session]);
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
    state = freshState();
    const page = await browser.newPage({ viewport: { width, height } });
    const errors = [];
    page.on('pageerror', e => errors.push(e.message));
    await page.route('**/*', handle);
    const connected = () => page.waitForFunction(() => document.getElementById('connection')?.textContent === 'Local daemon connected', null, { timeout: 5000 });

    // 1. Scan: the link opens the page at the Tailscale origin and the page
    //    exchanges the secret itself.
    await page.goto(origin + '/?visit=1#pair=pair-one');
    try { await connected(); } catch (e) {
      console.error('DEBUG connection =', await page.locator('#connection').textContent(), 'errors =', errors, 'exchanges =', state.exchanges, 'tokens =', state.tokensSeen.slice(0, 3));
      throw e;
    }
    assert.deepEqual(state.exchanges, ['pair-one']);
    assert.equal(await page.evaluate(() => location.hash), '', 'the secret is stripped from the address bar');
    assert.equal(await page.evaluate(() => localStorage.getItem('pc-device')), firstToken);
    assert.ok(state.tokensSeen.includes('Bearer ' + firstToken), 'the device token authorizes the API');
    assert.ok(state.sse >= 1, 'the event stream opened with the device token');
    assert.equal(await page.locator('#connection').textContent(), 'Local daemon connected');
    const overflow = await page.evaluate(() => ({ body: document.documentElement.scrollWidth, viewport: innerWidth }));
    assert.equal(overflow.body, width, 'no horizontal overflow');
    measurements.push({ step: 'paired', width, ...overflow, sse: state.sse });
    if (shots && width === 375) await page.screenshot({ path: shots + '/item2-phone-paired.png' });

    // 2. A replayed link is refused by the host, and the device keeps the token
    //    it already had instead of silently pairing again.
    await page.goto(origin + '/?visit=1#pair=pair-one');
    try { await connected(); } catch (e) {
      console.error('DEBUG connection =', await page.locator('#connection').textContent(), 'errors =', errors, 'exchanges =', state.exchanges, 'tokens =', state.tokensSeen.slice(0, 3));
      throw e;
    }
    assert.deepEqual(state.exchanges, ['pair-one'], 'a spent secret is not exchanged again');
    assert.equal(await page.evaluate(() => localStorage.getItem('pc-device')), firstToken,
      'a refused replay never replaces the token the device already has');
    assert.equal(await page.locator('#connection').textContent(), 'Local daemon connected');

    // 3. Revocation: the revoked device token stops working. The page must not
    //    pretend to be connected, and it must offer a way back in.
    state.revoked = true;
    await page.reload();
    await page.waitForFunction(() => document.getElementById('connection')?.textContent !== 'Local daemon connected', null, { timeout: 5000 });
    assert.notEqual(await page.locator('#connection').textContent(), 'Local daemon connected');
    const reconnect = page.getByRole('button', { name: /Reconnect|Connect/ });
    assert.ok(await reconnect.count() > 0, 'the page offers a way to reconnect');
    if (shots && width === 375) await page.screenshot({ path: shots + '/item2-phone-revoked.png' });
    measurements.push({ step: 'revoked', width, connection: await page.locator('#connection').textContent() });

    // 4. A fresh ticket pairs again, and the old token is replaced.
    state.revoked = false;
    await page.goto(origin + '/?visit=3#pair=pair-two');
    await connected();
    assert.equal(await page.evaluate(() => localStorage.getItem('pc-device')), secondToken);
    assert.ok(state.tokensSeen.includes('Bearer ' + secondToken));
    measurements.push({ step: 'repaired', width, sse: state.sse });

    assert.deepEqual(errors, []);
    console.log('pairing flow ' + width + 'x' + height + ' ok');
    await page.close();
  }
  if (shots) console.log(JSON.stringify(measurements, null, 1));
} finally {
  await browser.close();
}
