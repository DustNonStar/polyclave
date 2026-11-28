// Settings · Usage & limits: provider quotas, local usage, context vial, motion, theme and native keychain.
import { native } from '../api/sse.js';
import type { Api, Profile } from '../api/client.js';
import type { ClientKind, NotificationChannelKind } from '../generated/proto.js';
import { S } from '../state/store.js';
import type { Actions } from '../state/store.js';
import { button, compact, h } from './elements.js';
import { openDialog, segmented, textField } from './dialog.js';
import { icon } from './icons.js';
import { renderPairingMatrix } from './qr.js';
import { VENDORS, usageFor } from './statusbar.js';
import type { Intensity } from '../motion/spring.js';

type KeyStatus = { configured: boolean; masked_tail: string | null };
const VENDOR_COLOR: Record<string, string> = { claude: 'var(--v-claude)', codex: '#5b89d6', gemini: 'var(--v-gemini)', deepseek: 'var(--v-deepseek)' };

function meter(label: string, value: number | null, vendor: string): HTMLElement {
  const known = value !== null;
  return h('div', { class: 'meter', 'data-unavailable': String(!known) },
    h('div', { class: 'm-top' }, h('span', { text: label }), h('b', { text: known ? Math.round(value) + '%' : 'not reported' })),
    h('div', { class: 'bar' }, known ? h('i', { style: `--value:${value}%;--vendor:${VENDOR_COLOR[vendor] ?? 'var(--blue)'}`, 'data-high': String(value >= 80) }) : null));
}

function vendorCards(): HTMLElement {
  const grid = h('div', { class: 'vendor-cards' });
  for (const v of VENDORS) {
    const u = usageFor(v.id);
    const five = typeof u?.five_hour === 'number' ? u.five_hour : null;
    const week = typeof u?.weekly === 'number' ? u.weekly : null;
    const card = h('article', { class: 'card vendor-card' },
      h('div', { class: 'v-head' }, h('span', { class: 'dot', style: 'background:' + (VENDOR_COLOR[v.id] ?? 'var(--blue)') }), h('span', { class: 'v-name', text: v.name }), h('small', { text: u ? 'local ledger' : 'no data' })));
    if (v.id === 'deepseek') {
      card.append(h('div', { class: 'meter' }, h('div', { class: 'm-top' }, h('span', { text: 'Balance' }), h('b', { text: typeof u?.balance === 'number' ? '¥' + u.balance.toFixed(2) : 'not reported' }))));
    } else {
      card.append(meter('5-hour window', five, v.id), meter('Weekly limit', week, v.id));
    }
    card.append(h('div', { class: 'v-stats' },
      h('span', null, h('b', { text: u?.tokens ? compact(u.tokens) : '—' }), ' tokens · 24 h'),
      h('span', null, h('b', { text: typeof u?.cost === 'number' ? u.cost.toFixed(2) : '—' }), ' cost'),
      h('span', null, h('b', { text: u?.turns ? String(u.turns) : '—' }), ' turns')));
    grid.append(card);
  }
  return grid;
}

function usageBars(): HTMLElement {
  const series = S.usage.map(u => u.series ?? []).filter(s => s.length);
  const bars = h('div', { class: 'bars', role: 'img' });
  let caption: string;
  if (series.length) {
    const hours = Array.from({ length: 24 }, (_, i) => series.reduce((n, s) => n + (s[s.length - 24 + i] ?? 0), 0));
    const max = Math.max(1, ...hours);
    hours.forEach((v, i) => bars.append(h('div', { class: 'bar-col', title: v.toLocaleString() + ' tokens' }, h('i', { style: '--amount:' + (100 * v / max).toFixed(1) + '%' }), h('span', { text: i % 6 === 0 ? String(i - 23) + 'h' : '' }))));
    caption = 'Hourly tokens over the last 24 hours, all providers.';
  } else {
    const values = VENDORS.map(v => ({ v, n: usageFor(v.id)?.tokens ?? 0 }));
    const max = Math.max(1, ...values.map(x => x.n));
    for (const { v, n } of values) {
      bars.append(h('div', { class: 'bar-col', title: v.name + ': ' + (n ? n.toLocaleString() + ' tokens' : 'no data') },
        h('i', { style: `--amount:${n ? Math.max(2, 100 * n / max).toFixed(1) : 1}%;--vendor:${VENDOR_COLOR[v.id]}` }), h('span', { text: v.name + (n ? ' · ' + compact(n) : '') })));
    }
    caption = 'Tokens by provider over the last 24 hours, from the local ledger. Hourly series appear when the host reports them.';
  }
  bars.setAttribute('aria-label', caption);
  return h('div', { class: 'bars-24' }, bars, h('p', { class: 'field-hint', text: caption }));
}

function keyRows(act: Actions): HTMLElement {
  const rt = native();
  const wrap = h('div', { class: 'key-list', style: 'display:grid;gap:12px' });
  const apiProfiles: Profile[] = S.profiles.filter(p => p.kind === 'openai_compat' || p.kind === 'anthropic_compat');
  if (!rt) { wrap.append(h('p', { class: 'empty', text: 'Provider keys are stored in the system keychain by the desktop app. This browser tab never sees them.' })); return wrap; }
  if (!apiProfiles.length) { wrap.append(h('p', { class: 'empty', text: 'No API provider profiles are registered.' })); return wrap; }
  for (const p of apiProfiles) {
    const status = h('span', { class: 'key-status', text: 'Checking keychain…' });
    const show = (s: KeyStatus) => { status.textContent = s.configured ? 'Configured · ••••' + (s.masked_tail ?? '') : 'Not configured'; status.dataset.ok = String(s.configured); };
    void rt.core.invoke<KeyStatus>('key_status', { profile: p.id }).then(show).catch(() => { status.textContent = 'Keychain unavailable'; });
    const [fieldL, field] = textField('Replace key for ' + p.display_name, { type: 'password', autocomplete: 'off', mono: true, placeholder: 'Paste a new key' });
    const save = button([icon('key', 'ic--sm'), 'Save to keychain'], () => {
      const value = field.value.trim();
      field.value = '';
      if (!value) { status.textContent = 'Enter a key first'; return; }
      void rt.core.invoke<KeyStatus>('save_key', { profile: p.id, value }).then(s => { show(s); act.tell('Key saved for ' + p.display_name, 'info'); })
        .catch(e => { status.textContent = typeof e === 'string' ? e : 'The key was not saved'; });
    }, 'btn-primary');
    const remove = button('Remove', () => {
      void rt.core.invoke<KeyStatus>('delete_key', { profile: p.id }).then(show)
        .catch(() => { status.textContent = 'Removing keys needs a newer desktop build'; });
    }, 'btn-danger');
    wrap.append(h('div', { class: 'card key-row' }, h('div', { class: 'row' }, h('span', { class: 'card-title grow', text: p.display_name }), status), fieldL, h('div', { class: 'row actions' }, save, remove)));
  }
  wrap.append(h('div', { class: 'card key-row' },
    h('div', { class: 'row' }, h('span', { class: 'card-title grow', text: 'GitHub PR Gate & SCM Observer' }), h('span', { class: 'key-status', text: 'System keychain' })),
    h('p', { class: 'field-hint', text: 'PR Gate and SCM Observer read separate Bot and Reviewer tokens from the OS keychain under service "Polyclave.GitHub" (accounts "bot_token" and "reviewer_token"). Bot and Reviewer accounts must be distinct; tokens never appear in UI output or logs.' })));
  return wrap;
}

function remoteDevicesSection(api: Api, act: Actions): HTMLElement {
  const wrap = h('div', { style: 'display:grid;gap:12px' });
  const [labelL, labelInput] = textField('Device label', { placeholder: 'My iPhone', value: 'Mobile client' });
  let kind: ClientKind = 'ios';
  const kindSeg = segmented('Device platform', ['ios', 'android'] as const, kind, v => { kind = v; }, { ios: 'iOS', android: 'Android' });
  const qrSlot = h('div', { class: 'qr-slot' });
  const showQr = () => {
    qrSlot.replaceChildren();
    if (!S.lastPairing) return;
    const link = S.lastPairing.qr_uri;
    if (!link) {
      // Without a configured Tailscale HTTPS origin there is no address the phone
      // could open, so no link and no QR are handed out.
      qrSlot.append(h('div', { class: 'card qr-box', 'data-pairing': 'no-origin' },
        h('div', { style: 'display:grid;gap:6px;min-width:0' },
          h('span', { class: 'card-title', text: 'Pairing link unavailable' }),
          h('p', { class: 'field-hint', text: 'Set POLYCLAVE_TAILSCALE_ORIGIN to the https origin this daemon is served on (for example https://polyclave.tailnet.ts.net), restart the daemon, and create a new ticket. A link without it cannot reach this machine.' }),
          h('p', { class: 'field-hint', text: 'The ticket is valid until ' + S.lastPairing.ticket.expires_at + '.' }))));
      return;
    }
    qrSlot.append(h('div', { class: 'card qr-box' },
      renderPairingMatrix(link),
      h('div', { style: 'display:grid;gap:6px;min-width:0' },
        h('span', { class: 'card-title', text: '10-minute one-time pairing ticket' }),
        h('p', { class: 'field-hint mono', style: 'overflow-wrap:anywhere', text: link, 'data-pairing': 'link' }),
        h('p', { class: 'field-hint', text: 'Expires: ' + S.lastPairing.ticket.expires_at + ' · Scan it with the phone; the page exchanges the secret for a device token and strips it from the address bar.' }))));
  };
  showQr();
  const genBtn = button('Create 10-min QR', () => {
    genBtn.disabled = true;
    void api.createPairingTicket(kind, labelInput.value.trim() || 'Mobile client', 600).then(res => {
      S.lastPairing = res;
      showQr();
      act.tell('10-minute pairing ticket created', 'info');
    }).catch(act.problem).finally(() => { genBtn.disabled = false; });
  }, 'btn-primary', { 'data-focus-key': 'create-qr' });
  wrap.append(h('div', { class: 'card', style: 'display:grid;gap:10px' },
    kindSeg, labelL, h('div', { class: 'actions' }, genBtn), qrSlot));

  const devList = h('div', { style: 'display:grid;gap:8px' });
  if (!S.devices.length) {
    devList.append(h('p', { class: 'empty', text: 'No paired mobile or remote devices yet.' }));
  } else {
    for (const d of S.devices) {
      devList.append(h('div', { class: 'card row' },
        h('div', { class: 'grow', style: 'min-width:0' },
          h('div', { class: 'card-title', text: d.label + ' (' + d.client_kind + ')' }),
          h('p', { class: 'field-hint', text: (d.peer_identity ?? 'loopback') + (d.revoked_at ? ' · revoked' : ' · active') })),
        !d.revoked_at ? button('Revoke', () => {
          void api.revokeDevice(d.id).then(() => act.refresh()).catch(act.problem);
        }, 'btn-danger', { 'data-focus-key': 'rev-dev-' + d.id }) : null));
    }
  }
  wrap.append(devList);
  return wrap;
}

const MESH_CONCURRENCY_MAX = 16;
const MESH_CONCURRENCY_DEFAULT = 3;

/** Concurrent mesh Runs this node may have on this host: a − n + stepper that saves each step. */
function meshConcurrency(api: Api, act: Actions, nodeId: string): HTMLElement {
  const limit = S.meshLimits.find(l => l.node_id === nodeId);
  let value = limit?.max_concurrent_runs ?? MESH_CONCURRENCY_DEFAULT;
  let active = limit?.active_runs ?? 0;
  const out = h('output', { class: 'stepper-value mono', 'aria-live': 'polite' });
  const hint = h('p', { class: 'field-hint' });
  const show = () => {
    out.textContent = String(value);
    hint.textContent = active + ' running now · new Runs over the limit are refused, not queued · default ' + MESH_CONCURRENCY_DEFAULT + ', up to ' + MESH_CONCURRENCY_MAX;
    dec.disabled = value <= 1;
    inc.disabled = value >= MESH_CONCURRENCY_MAX;
  };
  const save = (next: number) => {
    if (next < 1 || next > MESH_CONCURRENCY_MAX) return;
    dec.disabled = inc.disabled = true;
    void api.setMeshConcurrency(nodeId, next)
      .then(res => {
        value = res.max_concurrent_runs; active = res.active_runs;
        S.meshLimits = S.meshLimits.filter(l => l.node_id !== nodeId).concat(res);
        act.tell('At most ' + value + ' concurrent Runs from this node', 'info');
      })
      .catch(act.problem)
      .finally(show);
  };
  const dec = button('\u2212', () => save(value - 1), 'stepper-btn', { 'aria-label': 'Fewer concurrent Runs', 'data-focus-key': 'mesh-dec-' + nodeId });
  const inc = button('+', () => save(value + 1), 'stepper-btn', { 'aria-label': 'More concurrent Runs', 'data-focus-key': 'mesh-inc-' + nodeId });
  show();
  return h('div', { class: 'mesh-cap' },
    h('div', { class: 'grow', style: 'min-width:0' }, h('div', { class: 'mesh-cap-label', text: 'Concurrent Runs' }), hint),
    h('div', { class: 'stepper', role: 'group', 'aria-label': 'Concurrent Runs from ' + nodeId }, dec, out, inc));
}

function meshSection(api: Api, act: Actions): HTMLElement {
  const wrap = h('div', { style: 'display:grid;gap:10px' });
  if (!S.meshNodes.length) {
    wrap.append(h('p', { class: 'empty', text: 'No AgentMesh peer nodes registered. Federated execution uses deny-by-default attenuated capability tokens.' }));
    return wrap;
  }
  for (const n of S.meshNodes) {
    wrap.append(h('div', { class: 'card mesh-node' },
      h('div', { class: 'row' },
        h('div', { class: 'grow', style: 'min-width:0' },
          h('div', { class: 'card-title', text: n.name + ' · ' + n.os_family + ' (' + n.status + ')' }),
          h('p', { class: 'field-hint mono', style: 'overflow-wrap:anywhere', text: n.endpoint + ' · repos: ' + (n.allowed_repos.join(', ') || 'none') })),
        n.status !== 'revoked' ? button('Revoke', () => {
          void api.revokeMeshNode(n.id).then(() => act.refresh()).catch(act.problem);
        }, 'btn-danger', { 'data-focus-key': 'rev-mesh-' + n.id }) : null),
      n.status !== 'revoked' ? meshConcurrency(api, act, n.id) : null));
  }
  return wrap;
}

function notifySection(api: Api, act: Actions): HTMLElement {
  const wrap = h('div', { style: 'display:grid;gap:10px' });
  let kind: NotificationChannelKind = 'ntfy';
  const kindSeg = segmented('Channel type', ['ntfy', 'unified_push'] as const, kind, v => { kind = v; }, { ntfy: 'Self-hosted ntfy', unified_push: 'UnifiedPush' });
  const [epL, ep] = textField('Private endpoint URL', { placeholder: 'http://127.0.0.1:8090', mono: true });
  const [topL, top] = textField('Topic', { placeholder: 'polyclave', value: 'polyclave', mono: true });
  const addBtn = button('Save channel', () => {
    const endpoint = ep.value.trim();
    if (!endpoint) { act.tell('Private endpoint URL is required'); return; }
    addBtn.disabled = true;
    void api.upsertNotificationChannel({ kind, label: kind === 'ntfy' ? 'Private ntfy' : 'UnifiedPush', endpoint, topic: top.value.trim() || 'polyclave', enabled: true })
      .then(() => { ep.value = ''; act.tell('Private notification channel saved', 'info'); return act.refresh(); })
      .catch(act.problem).finally(() => { addBtn.disabled = false; });
  }, 'btn-primary', { 'data-focus-key': 'save-notify' });
  const testBtn = button('Test wakeup', () => {
    testBtn.disabled = true;
    void api.testNotifications().then(r => act.tell('Delivered ' + r.delivered + ' private wakeup(s)', 'info')).catch(act.problem).finally(() => { testBtn.disabled = false; });
  }, 'btn-ghost', { 'data-focus-key': 'test-notify' });
  wrap.append(h('div', { class: 'card', style: 'display:grid;gap:10px' },
    h('p', { class: 'field-hint', text: 'Only loopback, private RFC1918, or Tailscale endpoints are permitted. Wakeups carry metadata only (no code or chat text).' }),
    kindSeg, epL, topL, h('div', { class: 'actions' }, addBtn, testBtn)));
  for (const ch of S.notificationChannels) {
    wrap.append(h('div', { class: 'card row' },
      h('div', { class: 'grow', style: 'min-width:0' },
        h('div', { class: 'card-title', text: ch.label + ' (' + ch.kind + ')' }),
        h('p', { class: 'field-hint mono', text: ch.endpoint + ' · ' + ch.topic }))));
  }
  return wrap;
}

function cuaSection(api: Api, act: Actions): HTMLElement {
  const wrap = h('div', { class: 'card', style: 'display:grid;gap:10px' });
  wrap.append(
    h('div', { class: 'row' },
      h('span', { class: 'card-title grow', text: S.cua.locked ? 'Desktop CUA lock held by ' + (S.cua.holder_run_id ?? 'agent') : 'Desktop CUA lock is free' }),
      h('span', { class: 'pill-tag', 'data-tone': S.cua.locked ? 'review' : 'done', text: S.cua.locked ? 'Locked' : 'Idle' })),
    h('p', { class: 'field-hint', text: 'Single-machine mutex. Acquires only after ' + S.cua.idle_threshold_secs + 's of user idle; click Reclaim Desktop anytime to take control back immediately.' }),
    h('div', { class: 'actions' },
      button('Reclaim desktop', () => {
        void api.reclaimCua().then(c => { S.cua = c; act.tell('Desktop reclaimed', 'info'); return act.refresh(); }).catch(act.problem);
      }, 'btn-danger', { 'data-focus-key': 'cua-reclaim' })));
  return wrap;
}

function evolutionAndSkillsSection(api: Api, act: Actions): HTMLElement {
  const wrap = h('div', { style: 'display:grid;gap:10px' });
  wrap.append(h('div', { class: 'card', style: 'display:grid;gap:8px' },
    h('p', { class: 'field-hint', text: 'Local-only distillation from SQLite history and Teach-Once versioned skills synced strictly to isolated vendor homes (never user global skills).' }),
    h('div', { class: 'actions' },
      button('Distill local suggestions', () => {
        void api.distillEvolution().then(async () => { act.tell('Local evolution suggestions distilled', 'info'); await act.refresh(); }).catch(act.problem);
      }, 'btn-primary', { 'data-focus-key': 'distill-evo' }))));
  for (const e of S.evolutionSuggestions.slice(0, 6)) {
    wrap.append(h('div', { class: 'card', style: 'display:grid;gap:6px' },
      h('div', { class: 'row' }, h('span', { class: 'card-title grow', text: e.title }), h('span', { class: 'pill-tag', text: e.status })),
      h('p', { class: 'field-hint', text: e.rationale }),
      h('div', { class: 'actions' },
        e.status === 'proposed' ? button('Apply', () => { void api.applyEvolution(e.id).then(() => act.refresh()).catch(act.problem); }, 'btn-ghost') : null,
        e.status === 'applied' ? button('Rollback', () => { void api.rollbackEvolution(e.id).then(() => act.refresh()).catch(act.problem); }, 'btn-danger') : null)));
  }
  const [nameL, nameIn] = textField('Teach-Once skill name', { placeholder: 'fast-verify', mono: true });
  const [bodyL, bodyIn] = textField('Skill instructions', { placeholder: '# Fast Verify\ncargo test --locked' });
  wrap.append(h('div', { class: 'card', style: 'display:grid;gap:8px' },
    nameL, bodyL,
    h('div', { class: 'actions' },
      button('Save & sync skill', () => {
        if (!nameIn.value.trim() || !bodyIn.value.trim()) { act.tell('Skill name and body required'); return; }
        void api.saveLocalSkill({ name: nameIn.value.trim(), description: 'Teach-Once workflow skill', body_md: bodyIn.value.trim() })
          .then(async () => { nameIn.value = ''; bodyIn.value = ''; act.tell('Skill saved and synced to isolated homes', 'info'); await act.refresh(); })
          .catch(act.problem);
      }, 'btn-primary', { 'data-focus-key': 'save-skill' }))));
  for (const sk of S.localSkills.slice(0, 4)) {
    wrap.append(h('div', { class: 'card row' },
      h('div', { class: 'grow' }, h('div', { class: 'card-title', text: sk.name + ' · v' + sk.version + (sk.enabled ? '' : ' (disabled)') }), h('p', { class: 'field-hint', text: sk.description })),
      sk.supersedes_id ? button('Rollback', () => { void api.rollbackLocalSkill(sk.id).then(() => act.refresh()).catch(act.problem); }, 'btn-ghost') : null));
  }
  return wrap;
}

function communitySection(api: Api, act: Actions): HTMLElement {
  const wrap = h('div', { class: 'card', style: 'display:grid;gap:10px' });
  const preview = h('p', { class: 'field-hint mono', text: 'Opt-in is disabled by default. Nothing is ever uploaded automatically.' });
  wrap.append(
    h('p', { class: 'field-hint', text: 'When explicitly enabled, Polyclave generates a locally redacted markdown draft (stripping paths, emails, and secrets) for your review.' }),
    h('div', { class: 'actions' },
      button('Enable & preview redacted draft', () => {
        void api.setCommunityOptIn(true).then(() => api.communityDraft()).then(d => {
          preview.textContent = d.title + ' (stripped ' + d.stripped_items + ' items): ' + d.redacted_markdown.slice(0, 140);
        }).catch(act.problem);
      }, 'btn-ghost', { 'data-focus-key': 'preview-community' })),
    preview);
  return wrap;
}

export function openSettings(api: Api, act: Actions, setTheme: (light: boolean) => void): void {
  const sections: [string, string, () => Node[]][] = [
    ['usage', 'Usage & limits', () => [vendorCards()]],
    ['devices', 'Devices & pairing', () => [remoteDevicesSection(api, act)]],
    ['mesh', 'AgentMesh nodes', () => [meshSection(api, act)]],
    ['notify', 'Private notifications', () => [notifySection(api, act)]],
    ['cua', 'CUA desktop lock', () => [cuaSection(api, act)]],
    ['evolution', 'Evolution & skills', () => [evolutionAndSkillsSection(api, act)]],
    ['community', 'Community draft', () => [communitySection(api, act)]],
    ['context', 'Context', () => {
      const ctx = S.usage.reduce((n, u) => Math.max(n, u.context ?? 0), 0);
      const canvas = h('canvas', { id: 'settings-vial', 'aria-hidden': 'true' });
      return [h('div', { class: 'card context-card' }, canvas, h('div', null,
        h('div', { class: 'big', text: ctx ? compact(ctx) + ' / 512K' : '— / 512K' }),
        h('p', { class: 'card-sub', text: ctx ? 'Largest prompt sent in the last 24 hours (' + (100 * ctx / 512000).toFixed(1) + '% of the window). Context use is separate from provider quota.' : 'No prompt sizes recorded in the last 24 hours.' })))];
    }],
    ['activity', '24-hour usage', () => [usageBars()]],
    ['motion', 'Motion', () => {
      let saved: string | null = null;
      try { saved = localStorage.getItem('pc-motion'); } catch { /* ignore */ }
      const current = (saved === 'off' || saved === 'subtle' || saved === 'ultra' ? saved : 'standard') as Intensity;
      return [segmented('Motion intensity', ['off', 'subtle', 'standard', 'ultra'] as const, current, v => {
        try { localStorage.setItem('pc-motion', v); } catch { /* ignore */ }
        document.dispatchEvent(new CustomEvent('pc-intensity', { detail: v }));
      }, { off: 'Off · 0fps', subtle: 'Subtle', standard: 'Standard', ultra: 'Ultra' }),
      h('p', { class: 'field-hint', text: 'Animation runs only while agents work and sleeps to 0 fps when idle. The system reduced-motion setting always wins.' })];
    }],
    ['appearance', 'Appearance', () => [segmented('Theme', ['dark', 'light'] as const, document.documentElement.dataset.theme === 'light' ? 'light' : 'dark', v => setTheme(v === 'light'), { dark: 'Deep Obsidian', light: 'Daylight Crystal' })]],
    ['keys', 'Provider keys', () => [keyRows(act)]],
  ];
  const main = h('div', { class: 'settings-main' });
  const nav = h('nav', { class: 'settings-nav', 'aria-label': 'Settings sections' });
  for (const [id, title, build] of sections) {
    main.append(h('section', { class: 'settings-section', id: 'set-' + id, 'aria-labelledby': 'set-h-' + id }, h('h3', { id: 'set-h-' + id, text: title }), ...build()));
    nav.append(button(title, () => {
      for (const b of nav.querySelectorAll('button')) b.removeAttribute('aria-current');
      nav.querySelector(`[data-target="${id}"]`)?.setAttribute('aria-current', 'true');
      document.getElementById('set-' + id)?.scrollIntoView({ block: 'start', behavior: matchMedia('(prefers-reduced-motion: reduce)').matches ? 'auto' : 'smooth' });
    }, undefined, { 'data-target': id }));
  }
  nav.querySelector('button')?.setAttribute('aria-current', 'true');
  openDialog('Settings · Usage & limits', [h('div', { class: 'settings' }, nav, main)], { wide: true });
  document.dispatchEvent(new Event('pc-settings'));
  const ctx = S.usage.reduce((n, u) => Math.max(n, u.context ?? 0), 0);
  document.dispatchEvent(new CustomEvent('pc-context', { detail: ctx / 512000 }));
}
