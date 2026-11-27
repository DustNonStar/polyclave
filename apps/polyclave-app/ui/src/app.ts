import './motion/index.js';
import { bootstrap, follow } from './api/sse.js';
import type { Frame } from './api/sse.js';
import { api as makeApi } from './api/client.js';
import { S, attentionPrGates, awaitingCouncils, needsReview, selectedSession } from './state/store.js';
import type { Actions, Panel, Pane } from './state/store.js';
import { byId } from './views/elements.js';
import { icon, setIcon } from './views/icons.js';
import { loadCollapsed, renderSidebar } from './views/sidebar.js';
import { renderChat, renderLive } from './views/chat.js';
import { mountModelSelect, renderModelSelect, showModelMenu } from './views/model-select.js';
import { layoutTopology, renderInspector } from './views/inspector.js';
import { renderStatus } from './views/statusbar.js';
import { openSettings } from './views/settings.js';
import { registerProject, tokenDialog } from './views/flows.js';

const api = makeApi(() => S.connection);
const stop = new AbortController();
const root = byId('app');
let reviewing = false;
let noticeTimer = 0;

/* ---------- theme ---------- */
function setTheme(light: boolean): void {
  document.documentElement.dataset.theme = light ? 'light' : 'dark';
  try { localStorage.setItem('pc-theme', light ? 'light' : 'dark'); } catch { /* storage unavailable */ }
  const b = byId('theme');
  setIcon(b, light ? 'moon' : 'sun');
  b.setAttribute('aria-label', light ? 'Dark theme' : 'Light theme');
  b.title = light ? 'Switch to Deep Obsidian' : 'Switch to Daylight Crystal';
}
let storedTheme: string | null = null;
try { storedTheme = localStorage.getItem('pc-theme'); } catch { /* ignore */ }
setTheme(storedTheme === 'light');

/* ---------- actions shared with views ---------- */
const act: Actions = {
  selectSession(id) {
    S.global = false; S.selected = id; S.activeRun = null; S.fileText = null; S.fileError = ''; S.memFilter = 'all';
    act.setPane('chat');
    void loadSession().catch(act.problem);
  },
  selectGlobal() {
    S.global = true; S.activeRun = null; S.messages = [];
    act.setPane('chat'); act.rerender();
  },
  openRun(id, focusPanel = false) {
    const run = S.runs.find(r => r.id === id);
    if (run && run.hub_session_id !== S.selected) { S.selected = run.hub_session_id; S.global = false; void loadSession().catch(act.problem); }
    S.activeRun = id; S.panel = 'Review';
    if (focusPanel) act.setPane('inspect');
    act.rerender();
  },
  setPanel(p: Panel) {
    S.panel = p;
    if (root.dataset.pane !== 'inspect' && matchMedia('(max-width: 859px)').matches) act.setPane('inspect');
    void renderInspector(api, act).catch(act.problem);
  },
  setPane(p: Pane) {
    S.pane = p; root.dataset.pane = p;
    for (const b of root.querySelectorAll<HTMLElement>('.mobile-nav button')) b.setAttribute('aria-pressed', String(b.dataset.pane === p));
    if (p === 'inspect') requestAnimationFrame(layoutTopology);
  },
  rerender() {
    renderSidebar(act); renderChat(api, act); renderModelSelect(); renderStatus();
    void renderInspector(api, act).catch(act.problem);
  },
  tell(message, tone = 'error') {
    byId('announcement').textContent = message;
    const n = byId('notice');
    n.dataset.tone = tone; n.replaceChildren(icon(tone === 'error' ? 'fail' : 'check', 'ic--sm'), document.createTextNode(message));
    n.hidden = false;
    clearTimeout(noticeTimer);
    noticeTimer = window.setTimeout(() => { n.hidden = true; }, tone === 'error' ? 8000 : 3200);
  },
  problem(e) { act.tell(e instanceof Error ? e.message : 'Local operation did not finish'); },
  refresh: () => sync(),
};

/* ---------- data ---------- */
async function loadSession(): Promise<void> {
  if (!S.connection || !S.selected) { act.rerender(); return; }
  const id = S.selected;
  const [messages, memory] = await Promise.all([api.messages(id), api.memory(id).catch(() => [])]);
  if (S.selected !== id) return;
  S.messages = messages; S.memory = memory;
  act.rerender();
}

async function sync(): Promise<void> {
  if (!S.connection) return;
  const [sessions, runs, profiles, tickets, usage, councils, prGates, pause, devices, meshNodes, meshLimits, notificationChannels, cua, evolutionSuggestions, localSkills] = await Promise.all([
    api.sessions(), api.runs(), api.profiles(), api.tickets().catch(() => S.tickets), api.usage().catch(() => S.usage),
    api.councils().catch(() => S.councils), api.prGates().catch(() => S.prGates), api.pauseState().catch(() => S.pause),
    api.devices().catch(() => S.devices), api.meshNodes().catch(() => S.meshNodes), api.meshLimits().catch(() => S.meshLimits), api.notificationChannels().catch(() => S.notificationChannels),
    api.cua().catch(() => S.cua), api.evolutionSuggestions().catch(() => S.evolutionSuggestions), api.localSkills().catch(() => S.localSkills),
  ]);
  S.sessions = sessions; S.runs = runs; S.profiles = profiles; S.tickets = tickets; S.usage = usage;
  S.councils = councils; S.prGates = prGates; S.pause = pause;
  S.devices = devices; S.meshNodes = meshNodes; S.meshLimits = meshLimits; S.notificationChannels = notificationChannels;
  S.cua = cua; S.evolutionSuggestions = evolutionSuggestions; S.localSkills = localSkills;
  if (!S.global && (!S.selected || !sessions.some(s => s.id === S.selected))) S.selected = sessions.find(s => s.role === 'hub')?.id ?? null;
  if (S.selected) {
    const id = S.selected;
    const [messages, memory] = await Promise.all([api.messages(id), api.memory(id).catch(() => S.memory)]);
    if (S.selected === id) { S.messages = messages; S.memory = memory; }
  } else { S.messages = []; S.memory = []; }
  act.rerender();
  const waiting = needsReview().some(r => S.global || r.hub_session_id === S.selected);
  if (waiting && !reviewing) document.dispatchEvent(new Event('pc-review'));
  if (!waiting && reviewing && !S.live.size) document.dispatchEvent(new Event('pc-idle'));
  reviewing = waiting;
}

let pending = 0;
function queueSync(): void {
  if (pending) return;
  pending = window.setTimeout(() => { pending = 0; void sync().catch(act.problem); }, 60);
}

type AgentFrame = { run_id: string; message: { kind: string; text?: string; success?: boolean } };
async function frame(f: Frame): Promise<void> {
  if (f.event === 'resync_required') { await sync(); return; }
  if (f.event === 'change') { queueSync(); return; }
  if (f.event !== 'agent') return;
  const e = f.data as AgentFrame;
  const id = e.run_id;
  const kind = e.message.kind;
  if (kind === 'text_delta' || kind === 'reasoning_delta') {
    const live = S.live.get(id) ?? { text: '', reasoning: '' };
    const first = !S.live.has(id);
    if (kind === 'text_delta') live.text += e.message.text ?? ''; else live.reasoning += e.message.text ?? '';
    S.live.set(id, live);
    if (id === S.selected && !S.global) {
      renderLive();
      if (kind === 'text_delta') document.dispatchEvent(new Event('pc-stream'));
      else if (first) document.dispatchEvent(new Event('pc-start'));
      const scroller = byId('history-scroll');
      if (scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight < 120) scroller.scrollTop = scroller.scrollHeight;
    } else if (first) document.dispatchEvent(new Event('pc-stream'));
    if (first) { renderSidebar(act); renderStatus(); }
    return;
  }
  if (kind === 'finished') {
    S.live.delete(id);
    if (id === S.selected) renderLive();
    if (!S.live.size) document.dispatchEvent(new Event(reviewing ? 'pc-review' : 'pc-idle'));
    renderSidebar(act); renderStatus();
    if (e.message.success === false && id === S.selected) act.tell('The turn ended without finishing. Check the provider and try again.');
    queueSync();
  }
}

/* ---------- static controls ---------- */
setIcon(byId('register'), 'plus');
setIcon(byId('usage'), 'gear', 'Settings · Usage & limits');
byId('global-hub').prepend(icon('globe', 'ic--sm'));
byId('global-hub').querySelector('.dot')?.remove();
setIcon(byId('send'), 'send');
setIcon(byId('cancel'), 'stop');
byId('live-think').querySelector('summary')?.append(icon('chevronDown', 'ic--sm'));

byId('theme').addEventListener('click', () => setTheme(document.documentElement.dataset.theme !== 'light'));
byId('register').addEventListener('click', () => registerProject(api, act));
byId('global-hub').addEventListener('click', () => act.selectGlobal());
byId('needs-you').addEventListener('click', () => {
  const firstRun = needsReview()[0];
  if (firstRun) { S.reviewView = 'runs'; act.openRun(firstRun.id, true); return; }
  const firstCouncil = awaitingCouncils()[0];
  if (firstCouncil) { S.activeCouncil = firstCouncil.id; S.reviewView = 'council'; act.setPanel('Review'); return; }
  const firstGate = attentionPrGates()[0];
  if (firstGate) { S.activePrGate = firstGate.id; S.reviewView = 'pr_gates'; act.setPanel('Review'); return; }
  act.tell('Nothing needs your review right now', 'info');
});
byId('pause-toggle').addEventListener('click', () => {
  const b = byId<HTMLButtonElement>('pause-toggle');
  b.disabled = true;
  const op = S.pause.paused ? api.resume() : api.pause('Paused from Polyclave UI');
  void op.then(state => {
    S.pause = state;
    act.tell(state.paused ? 'Host globally paused' : 'Host resumed', 'info');
    return sync();
  }).catch(act.problem).finally(() => { b.disabled = false; });
});
byId('usage').addEventListener('click', () => {
  void api.usage().then(v => { S.usage = v; renderStatus(); }).catch(() => {}).finally(() => openSettings(api, act, setTheme));
});
for (const b of root.querySelectorAll<HTMLButtonElement>('.mobile-nav button')) b.addEventListener('click', () => act.setPane((b.dataset.pane ?? 'chat') as Pane));
byId('model-pill').addEventListener('click', () => showModelMenu());
byId('memory-chip').addEventListener('click', () => act.setPanel('Memory'));
byId('auto').addEventListener('click', () => {
  const b = byId('auto');
  const on = b.getAttribute('aria-pressed') !== 'true';
  b.setAttribute('aria-pressed', String(on));
  b.textContent = on ? 'Auto' : 'Manual';
  b.title = on ? 'The Hub may propose tickets on its own' : 'The Hub waits for explicit tickets';
});

const prompt = byId<HTMLTextAreaElement>('prompt');
const grow = () => { prompt.style.height = 'auto'; prompt.style.height = Math.min(200, prompt.scrollHeight) + 'px'; };
prompt.addEventListener('input', grow);
prompt.addEventListener('keydown', e => {
  if (e.key === 'Enter' && !e.shiftKey && !e.isComposing) { e.preventDefault(); byId<HTMLFormElement>('compose').requestSubmit(); }
});
byId('compose').addEventListener('submit', e => {
  e.preventDefault();
  const text = prompt.value.trim();
  const s = selectedSession();
  if (!text || !s || S.global) return;
  if (!S.connection) { act.tell('Connect to the local daemon first'); return; }
  void api.send(s.id, text).then(() => {
    if (prompt.value.trim() === text) { prompt.value = ''; grow(); }
    if (!S.live.has(s.id)) { S.live.set(s.id, { text: '', reasoning: '' }); renderLive(); }
    document.dispatchEvent(new Event('pc-start'));
    queueSync();
  }).catch(act.problem);
});
byId('cancel').addEventListener('click', () => {
  if (S.selected) void api.cancelTurn(S.selected).then(() => act.tell('Stopping the turn', 'info')).catch(act.problem);
});

mountModelSelect(o => {
  const s = selectedSession();
  if (!s) return;
  void api.switchProfile(s.id, o.profile.id, o.model).then(() => { act.tell('Model set to ' + o.label, 'info'); return sync(); }).catch(act.problem);
});

/* ---------- start ---------- */
loadCollapsed();
act.rerender();
void (async () => {
  try { S.connection = await bootstrap(); } catch (e) { act.problem(e); }
  if (!S.connection) {
    byId('connection').textContent = 'Local daemon not connected';
    renderStatus(); tokenDialog(act); return;
  }
  try { await sync(); } catch (e) { byId('connection').textContent = 'Local daemon unreachable'; act.problem(e); renderStatus(); return; }
  S.online = true;
  byId('connection').textContent = 'Local daemon connected';
  renderStatus(); renderSidebar(act);
  void follow(S.connection, frame, stop.signal);
})();
window.addEventListener('pagehide', () => stop.abort(), { once: true });
