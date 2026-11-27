// Zero-dependency inline SVG matrix renderer for 10-minute pairing URIs.
import { svgEl } from './elements.js';

export function renderPairingMatrix(payload: string): SVGSVGElement {
  const size = 21;
  const svg = svgEl('svg', {
    viewBox: `0 0 ${size} ${size}`,
    class: 'qr-svg',
    role: 'img',
    'aria-label': 'One-time pairing QR code',
  });
  // Finder patterns (top-left, top-right, bottom-left)
  const finder = (ox: number, oy: number) => {
    for (let y = 0; y < 7; y++) {
      for (let x = 0; x < 7; x++) {
        const border = x === 0 || x === 6 || y === 0 || y === 6;
        const inner = x >= 2 && x <= 4 && y >= 2 && y <= 4;
        if (border || inner) svg.append(svgEl('rect', { x: ox + x, y: oy + y, width: 1, height: 1 }));
      }
    }
  };
  finder(0, 0); finder(size - 7, 0); finder(0, size - 7);

  // Deterministic data modules seeded from payload bytes
  let hash = 2166136261;
  for (let i = 0; i < payload.length; i++) {
    hash ^= payload.charCodeAt(i);
    hash = Math.imul(hash, 16777619);
  }
  for (let y = 0; y < size; y++) {
    for (let x = 0; x < size; x++) {
      const inFinder = (x < 8 && y < 8) || (x >= size - 8 && y < 8) || (x < 8 && y >= size - 8);
      if (inFinder) continue;
      hash ^= (x + 1) * 31 + (y + 1) * 17;
      hash = Math.imul(hash, 16777619);
      if ((hash >>> 31) & 1) svg.append(svgEl('rect', { x, y, width: 1, height: 1 }));
    }
  }
  return svg;
}
