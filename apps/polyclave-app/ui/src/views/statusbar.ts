// Bottom status bar: connection, run counts and the four vendor quota rings.
import type { ProviderUsage } from '../api/client.js';
import { S, vendorOf } from '../state/store.js';
import { byId, h, svgEl } from './elements.js';

export const VENDORS = [
  { id: 'claude', name: 'Claude' },
  { id: 'codex', name: 'Codex' },
  { id: 'gemini', name: 'Gemini' },
  { id: 'deepseek', name: 'DeepSeek' },
] as const;

export const usageFor = (vendor: string): ProviderUsage | undefined => S.usage.find(u => vendorOf(u.vendor) === vendor);
const pct = (v: number | null | undefined): number | null => (typeof v === 'number' && Number.isFinite(v) ? Math.max(0, Math.min(100, v)) : null);

function ring(vendor: string): SVGSVGElement {
  const u = usageFor(vendor);
  const svg = svgEl('svg', { viewBox: '0 0 20 20', 'aria-hidden': 'true' });
  if (vendor === 'deepseek') {
    // No 5-hour window: a solid dot.
    svg.append(svgEl('circle', { class: 'q-solid', cx: 10, cy: 10, r: 6.5, opacity: u ? 1 : 0.4 }));
    return svg;
  }
  const five = pct(u?.five_hour), week = pct(u?.weekly);
  const arc = (r: number, value: number | null) => {
    if (value === null) { svg.append(svgEl('circle', { class: 'q-off', cx: 10, cy: 10, r })); return; }
    const c = 2 * Math.PI * r;
    svg.append(svgEl('circle', { class: 'q-track', cx: 10, cy: 10, r }));
    svg.append(svgEl('circle', { class: 'q-arc', cx: 10, cy: 10, r, 'stroke-dasharray': `${(c * value / 100).toFixed(2)} ${c.toFixed(2)}` }));
  };
  arc(8, five); arc(4.6, week);
  return svg;
}

export function renderStatus(): void {
  const conn = byId('conn-dot');
  conn.dataset.state = S.online ? 'online' : S.connection ? 'connecting' : 'offline';
  const pauseBtn = document.getElementById('pause-toggle');
  if (pauseBtn) {
    pauseBtn.setAttribute('aria-pressed', String(S.pause.paused));
    pauseBtn.dataset.paused = String(S.pause.paused);
    pauseBtn.textContent = S.pause.paused ? 'Paused · Resume' : 'Pause';
    pauseBtn.title = S.pause.paused
      ? (S.pause.reason ? 'Paused: ' + S.pause.reason + ' — click to resume' : 'Host globally paused — click to resume')
      : 'Pause all dispatches and auto-merges';
  }
  const count = (status: string) => S.runs.filter(r => r.status === status).length;
  const thinking = [...S.live.keys()].filter(k => S.sessions.some(s => s.id === k)).length;
  const items: [string, number, string][] = [
    ['run', count('running'), 'running'], ['think', thinking, 'thinking'], ['review', count('needs_review'), 'to review'], ['done', count('done'), 'done'],
  ];
  const countNodes: Node[] = items.map(([tone, n, label]) =>
    h('span', { class: 'count', title: n + ' ' + label }, h('span', { class: 'dot dot--' + tone }), h('b', { text: String(n) }), h('span', { class: 'count-label', text: label })));
  if (S.pause.paused) {
    countNodes.unshift(h('span', { class: 'pill-tag status-paused', 'data-tone': 'review', text: 'Paused' }));
  }
  byId('counts').replaceChildren(...countNodes);

  const quota = byId('quota');
  quota.replaceChildren(...VENDORS.map(v => {
    const u = usageFor(v.id);
    const five = pct(u?.five_hour), week = pct(u?.weekly);
    const value = v.id === 'deepseek'
      ? (typeof u?.balance === 'number' ? '¥' + u.balance.toFixed(2) : '—')
      : five !== null ? Math.round(five) + '%' : '—';
    const title = v.id === 'deepseek'
      ? v.name + (typeof u?.balance === 'number' ? ' · balance ' + u.balance.toFixed(2) : ' · balance not reported') + (u?.tokens ? ' · ' + u.tokens.toLocaleString() + ' local tokens (24 h)' : '')
      : v.name + ' · 5-hour ' + (five !== null ? five + '%' : 'not reported') + ' · weekly ' + (week !== null ? week + '%' : 'not reported');
    return h('span', { class: 'q', 'data-vendor': v.id, title, role: 'img', 'aria-label': title },
      ring(v.id), h('span', { class: 'q-label', text: v.name }), h('span', { class: 'q-val', text: value }));
  }));
}
