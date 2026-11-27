// Original line icons on a 24-unit grid; stroke only, coloured by currentColor.
const paths = {
  plus: 'M12 5v14 M5 12h14',
  moon: 'M19.5 14.5A8 8 0 0 1 9.5 4.5a8 8 0 1 0 10 10z',
  sun: 'M12 3v2 M12 19v2 M3 12h2 M19 12h2 M5.6 5.6l1.4 1.4 M17 17l1.4 1.4 M5.6 18.4L7 17 M17 7l1.4-1.4 M16 12a4 4 0 1 1-8 0a4 4 0 0 1 8 0z',
  gear: 'M12 15a3 3 0 1 0 0-6a3 3 0 0 0 0 6z M19 12a7 7 0 0 0-.1-1.2l2-1.6-2-3.4-2.4 1a7 7 0 0 0-2-1.2L14 3h-4l-.5 2.6a7 7 0 0 0-2 1.2l-2.4-1-2 3.4 2 1.6A7 7 0 0 0 5 12c0 .4 0 .8.1 1.2l-2 1.6 2 3.4 2.4-1a7 7 0 0 0 2 1.2L10 21h4l.5-2.6a7 7 0 0 0 2-1.2l2.4 1 2-3.4-2-1.6c.1-.4.1-.8.1-1.2z',
  chevronDown: 'M6 9l6 6 6-6',
  chevronRight: 'M9 6l6 6-6 6',
  send: 'M12 19V5 M6 11l6-6 6 6',
  stop: 'M8 8h8v8H8z',
  check: 'M5 12.5l4.2 4.2L19 7',
  close: 'M6 6l12 12 M18 6L6 18',
  retry: 'M4 12a8 8 0 1 0 2.4-5.7 M4 4v5h5',
  globe: 'M21 12a9 9 0 1 1-18 0a9 9 0 0 1 18 0z M3 12h18 M12 3c-4 4.5-4 13.5 0 18 M12 3c4 4.5 4 13.5 0 18',
  bell: 'M6 16V11a6 6 0 1 1 12 0v5l1.5 2h-15z M10 20.5a2 2 0 0 0 4 0',
  topology: 'M12 4v5 M12 9l-6 6 M12 9l6 6 M10 3h4v4h-4z M4 15h4v4H4z M16 15h4v4h-4z',
  review: 'M6 3h9l4 4v14H6z M9 13l2 2 4-4',
  file: 'M6 3h8l4 4v14H6z M14 3v4h4',
  memory: 'M6 4h12v16H6z M9 8h6 M9 12h6 M9 16h4',
  brain: 'M9 5a3 3 0 0 0-3 3a3 3 0 0 0-1 5.6A3 3 0 0 0 9 19h1V5z M15 5a3 3 0 0 1 3 3a3 3 0 0 1 1 5.6A3 3 0 0 1 15 19h-1V5z',
  tool: 'M14.5 6.5a4 4 0 0 0 5 5l-8 8a2.1 2.1 0 0 1-3-3z',
  bolt: 'M13 3L5 13h6l-1 8 8-10h-6z',
  branch: 'M7 4v16 M7 8c0 4 10 2 10 7 M17 15v5',
  chat: 'M5 5h14v10H10l-5 4z',
  ticket: 'M4 7h16v3a2 2 0 0 0 0 4v3H4v-3a2 2 0 0 0 0-4z M14 7v10',
  fail: 'M12 3a9 9 0 1 0 0 18a9 9 0 0 0 0-18z M9 9l6 6 M15 9l-6 6',
  folder: 'M3 7h7l2 2h9v10H3z',
  arrowRight: 'M5 12h14 M13 6l6 6-6 6',
  key: 'M14 10a4 4 0 1 0-3.2 3.9L13 16h2v2h2v2h3v-3l-6.1-6.1c.1-.3.1-.6.1-.9z',
} as const;
export type IconName = keyof typeof paths;

export function icon(name: IconName, cls = ''): SVGSVGElement {
  const ns = 'http://www.w3.org/2000/svg';
  const svg = document.createElementNS(ns, 'svg');
  svg.setAttribute('viewBox', '0 0 24 24');
  svg.setAttribute('aria-hidden', 'true');
  svg.setAttribute('class', ('ic ' + cls).trim());
  const path = document.createElementNS(ns, 'path');
  path.setAttribute('d', paths[name]);
  svg.append(path);
  return svg;
}

/** Replace a control's contents with exactly one icon (no duplicated glyphs). */
export function setIcon(target: HTMLElement, name: IconName, label?: string): void {
  target.replaceChildren(icon(name));
  if (label) target.append(document.createTextNode(label));
}
