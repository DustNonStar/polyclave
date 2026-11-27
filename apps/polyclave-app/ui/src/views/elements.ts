// DOM construction helpers. All text goes through textContent, so renderer input is never parsed as HTML.
type Attrs = Record<string, string | number | boolean | null | undefined | EventListener>;
type Child = Node | string | null | undefined | false;

export function byId<T extends HTMLElement = HTMLElement>(id: string): T {
  const node = document.getElementById(id);
  if (!node) throw Error('Missing ' + id);
  return node as T;
}

export function h<K extends keyof HTMLElementTagNameMap>(tag: K, attrs?: Attrs | null, ...children: Child[]): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  if (attrs) {
    for (const [key, value] of Object.entries(attrs)) {
      if (value === undefined || value === null || value === false) continue;
      if (key.startsWith('on') && typeof value === 'function') node.addEventListener(key.slice(2), value);
      else if (key === 'class') node.className = String(value);
      else if (key === 'text') node.textContent = String(value);
      else if (key.startsWith('data-') || key.startsWith('aria-') || key === 'role' || key === 'title' || key === 'for' || key === 'tabindex') node.setAttribute(key, value === true ? 'true' : String(value));
      else (node as unknown as Record<string, unknown>)[key] = value;
    }
  }
  append(node, children);
  return node;
}

export function append(node: Node, children: Child[]): void {
  for (const child of children) {
    if (child === null || child === undefined || child === false) continue;
    node.appendChild(typeof child === 'string' ? document.createTextNode(child) : child);
  }
}

export function el<K extends keyof HTMLElementTagNameMap>(tag: K, text?: string, cls?: string): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  if (text !== undefined) node.textContent = text;
  if (cls) node.className = cls;
  return node;
}

export function button(text: string | Child[], action: () => void, cls?: string, attrs?: Attrs): HTMLButtonElement {
  const b = h('button', { type: 'button', class: cls, ...attrs });
  if (typeof text === 'string') b.textContent = text;
  else append(b, text);
  b.addEventListener('click', action);
  return b;
}

/** Render text, turning `inline code` spans into neutral code chips. */
export function richText(text: string): DocumentFragment {
  const out = document.createDocumentFragment();
  const parts = text.split(/(`[^`\n]{1,200}`)/g);
  for (const part of parts) {
    if (part.length > 2 && part.startsWith('`') && part.endsWith('`')) out.append(h('code', { class: 'inline', text: part.slice(1, -1) }));
    else if (part) out.append(document.createTextNode(part));
  }
  return out;
}

export function svgEl<K extends keyof SVGElementTagNameMap>(tag: K, attrs: Record<string, string | number> = {}): SVGElementTagNameMap[K] {
  const node = document.createElementNS('http://www.w3.org/2000/svg', tag);
  for (const [k, v] of Object.entries(attrs)) node.setAttribute(k, String(v));
  return node;
}

export const short = (sha: string | null | undefined, n = 7): string => (sha ? sha.slice(0, n) : '—');
export const basename = (path: string): string => path.replace(/[\\/]+$/, '').split(/[\\/]/).pop() || path;
export function compact(n: number): string {
  if (n >= 1e6) return (n / 1e6).toFixed(n >= 1e7 ? 0 : 1) + 'M';
  if (n >= 1e3) return (n / 1e3).toFixed(n >= 1e4 ? 0 : 1) + 'K';
  return String(n);
}
