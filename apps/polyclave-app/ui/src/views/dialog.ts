// Modal dialog and form field helpers built on the native <dialog> element.
import { byId, button, h } from './elements.js';
import { icon } from './icons.js';

const dialog = () => byId<HTMLDialogElement>('modal');

export interface DialogOptions { wide?: boolean; footer?: Node[]; onClose?: () => void }

export function openDialog(title: string, body: Node[], opts: DialogOptions = {}): HTMLDialogElement {
  const d = dialog();
  const content = byId('dialog-content');
  const close = button([icon('close')], () => closeDialog(), 'icon-btn', { 'aria-label': 'Close' });
  const head = h('header', { class: 'dlg-head' }, h('h2', { text: title, id: 'dialog-title' }), close);
  const nodes: Node[] = [head];
  if (opts.wide) nodes.push(...body);
  else nodes.push(h('div', { class: 'dlg-body' }, ...body));
  if (opts.footer?.length) nodes.push(h('footer', { class: 'dlg-foot' }, ...opts.footer));
  content.replaceChildren(...nodes);
  d.dataset.size = opts.wide ? 'wide' : 'normal';
  d.setAttribute('aria-labelledby', 'dialog-title');
  d.onclose = () => { opts.onClose?.(); };
  if (!d.open) d.showModal();
  return d;
}

export function closeDialog(): void { const d = dialog(); if (d.open) d.close(); }

// Close on backdrop click.
document.addEventListener('click', event => {
  const d = document.getElementById('modal');
  if (d instanceof HTMLDialogElement && event.target === d) d.close();
});

export function textField(label: string, opts: { value?: string; type?: string; placeholder?: string; hint?: string; mono?: boolean; autocomplete?: string } = {}): [HTMLLabelElement, HTMLInputElement] {
  const input = h('input', { type: opts.type ?? 'text', value: opts.value ?? '', placeholder: opts.placeholder ?? '', autocomplete: opts.autocomplete ?? 'off', spellcheck: false });
  if (opts.mono) input.style.fontFamily = 'var(--mono)';
  const l = h('label', { class: 'field-label' }, h('span', { text: label }), input, opts.hint ? h('span', { class: 'field-hint', text: opts.hint }) : null);
  return [l, input];
}

export function areaField(label: string, opts: { value?: string; placeholder?: string; hint?: string; rows?: number } = {}): [HTMLLabelElement, HTMLTextAreaElement] {
  const area = h('textarea', { class: 'field', rows: opts.rows ?? 4, placeholder: opts.placeholder ?? '' });
  area.value = opts.value ?? '';
  const l = h('label', { class: 'field-label' }, h('span', { text: label }), area, opts.hint ? h('span', { class: 'field-hint', text: opts.hint }) : null);
  return [l, area];
}

/** Accessible single-choice segmented control (radiogroup). */
export function segmented<T extends string>(label: string, values: readonly T[], current: T, onChange: (v: T) => void, names?: Partial<Record<T, string>>): HTMLElement {
  const group = h('div', { class: 'seg', role: 'radiogroup', 'aria-label': label });
  const set = (v: T) => {
    for (const b of group.querySelectorAll<HTMLButtonElement>('button')) {
      const on = b.dataset.value === v;
      b.setAttribute('aria-checked', String(on)); b.tabIndex = on ? 0 : -1;
    }
  };
  values.forEach((v, i) => {
    const b = button(names?.[v] ?? v, () => { set(v); onChange(v); }, undefined, { role: 'radio', 'data-value': v });
    b.addEventListener('keydown', e => {
      const step = e.key === 'ArrowRight' || e.key === 'ArrowDown' ? 1 : e.key === 'ArrowLeft' || e.key === 'ArrowUp' ? -1 : 0;
      if (!step) return;
      e.preventDefault();
      const next = values[(i + step + values.length) % values.length];
      if (next === undefined) return;
      set(next); onChange(next);
      group.querySelector<HTMLButtonElement>(`button[data-value="${next}"]`)?.focus();
    });
    group.append(b);
  });
  set(current);
  return group;
}

export function selectField<T extends string>(label: string, values: readonly T[], current: T): [HTMLLabelElement, HTMLSelectElement] {
  const select = h('select', { class: 'field' });
  for (const v of values) select.add(new Option(v.replaceAll('_', ' '), v));
  select.value = current;
  return [h('label', { class: 'field-label' }, h('span', { text: label }), select), select];
}
