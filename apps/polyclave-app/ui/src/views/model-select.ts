// Custom provider/model listbox. DeepSeek API is listed first, with deepseek-flash (its default) before deepseek-v4-pro.
import { S, selectedSession } from '../state/store.js';
import type { Profile } from '../api/client.js';
import { byId, h } from './elements.js';
import { icon } from './icons.js';

export interface ModelOption { value: string; label: string; profile: Profile; model: string }

const DEEPSEEK_ORDER = ['deepseek-flash', 'deepseek-v4-pro'];

export function modelOptions(profiles: Profile[]): ModelOption[] {
  const sorted = [...profiles].sort((a, b) => (a.id === 'deepseek-api' ? -1 : b.id === 'deepseek-api' ? 1 : 0));
  const out: ModelOption[] = [];
  for (const p of sorted) {
    const models = p.id === 'deepseek-api'
      ? [...p.models].sort((a, b) => rank(a) - rank(b))
      : p.models;
    for (const m of models) {
      const suffix = m === 'default' ? '' : ' · ' + m + (p.id === 'deepseek-api' && m === 'deepseek-flash' ? ' (default)' : '');
      out.push({ value: p.id + '|' + m, label: p.display_name + suffix, profile: p, model: m });
    }
  }
  return out;
}
function rank(model: string): number { const i = DEEPSEEK_ORDER.indexOf(model); return i < 0 ? DEEPSEEK_ORDER.length : i; }

export function currentModelLabel(): string {
  const s = selectedSession();
  if (!s) return 'Choose model';
  const o = modelOptions(S.profiles).find(x => x.value === s.profile_id + '|' + s.provider_model);
  return o?.label ?? (s.profile_id === 'builtin-hub' ? 'Local hub' : s.provider_model);
}

let open = false;
let activeIndex = -1;
let onPick: (o: ModelOption) => void = () => {};

export function mountModelSelect(pick: (o: ModelOption) => void): void {
  onPick = pick;
  const root = byId('provider');
  const trigger = h('button', { type: 'button', class: 'model-trigger', id: 'provider-trigger', 'aria-haspopup': 'listbox', 'aria-expanded': 'false', 'aria-controls': 'provider-menu', 'aria-label': 'Model provider' });
  const menu = h('div', { class: 'model-menu', id: 'provider-menu', role: 'listbox', 'aria-label': 'Provider and model', tabindex: '-1' });
  menu.hidden = true;
  root.replaceChildren(trigger, menu);
  trigger.addEventListener('click', () => (open ? close() : show()));
  trigger.addEventListener('keydown', e => { if (e.key === 'ArrowDown' || e.key === 'ArrowUp') { e.preventDefault(); show(); } });
  menu.addEventListener('keydown', e => {
    const items = options();
    if (e.key === 'Escape') { e.preventDefault(); close(); trigger.focus(); return; }
    if (e.key === 'Tab') { close(); return; }
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      e.preventDefault();
      activeIndex = (activeIndex + (e.key === 'ArrowDown' ? 1 : -1) + items.length) % items.length;
      highlight();
    } else if (e.key === 'Home') { e.preventDefault(); activeIndex = 0; highlight(); }
    else if (e.key === 'End') { e.preventDefault(); activeIndex = items.length - 1; highlight(); }
    else if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); items[activeIndex]?.click(); }
  });
  document.addEventListener('pointerdown', e => { if (open && !root.contains(e.target as Node) && !(e.target as HTMLElement).closest?.('#model-pill')) close(); });
  renderModelSelect();
}

const options = () => [...byId('provider-menu').querySelectorAll<HTMLElement>('[role="option"]')];
function highlight(): void {
  options().forEach((o, i) => {
    o.dataset.active = String(i === activeIndex);
    if (i === activeIndex) { o.scrollIntoView({ block: 'nearest' }); byId('provider-menu').setAttribute('aria-activedescendant', o.id); }
  });
}

export function showModelMenu(): void { show(); }
function show(): void {
  if (!selectedSession()) return;
  open = true;
  renderModelSelect();
  const menu = byId('provider-menu');
  menu.hidden = false;
  byId('provider-trigger').setAttribute('aria-expanded', 'true');
  const items = options();
  activeIndex = Math.max(0, items.findIndex(o => o.getAttribute('aria-selected') === 'true'));
  highlight();
  menu.focus({ preventScroll: true });
}
function close(): void {
  open = false;
  const menu = document.getElementById('provider-menu');
  if (menu) menu.hidden = true;
  document.getElementById('provider-trigger')?.setAttribute('aria-expanded', 'false');
}

export function renderModelSelect(): void {
  const trigger = document.getElementById('provider-trigger');
  const menu = document.getElementById('provider-menu');
  if (!trigger || !menu) return;
  const s = selectedSession();
  trigger.replaceChildren(h('span', { class: 'label', text: currentModelLabel() }), icon('chevronDown', 'ic--sm'));
  (trigger as HTMLButtonElement).disabled = !s;
  const current = s ? s.profile_id + '|' + s.provider_model : '';
  const list = modelOptions(S.profiles);
  menu.replaceChildren();
  let group = '';
  list.forEach((o, i) => {
    if (o.profile.display_name !== group) {
      group = o.profile.display_name;
      menu.append(h('div', { class: 'model-group', role: 'presentation', text: group }));
    }
    const item = h('div', {
      class: 'model-option', role: 'option', id: 'model-opt-' + i, 'data-value': o.value, 'aria-selected': String(o.value === current),
      onclick: () => { close(); byId('provider-trigger').focus(); if (o.value !== current) onPick(o); },
    }, h('span', { class: 'label', text: o.label }), icon('check', 'ic--sm check'));
    menu.append(item);
  });
  if (!list.length) menu.append(h('div', { class: 'model-group', text: 'No registered providers' }));
}
