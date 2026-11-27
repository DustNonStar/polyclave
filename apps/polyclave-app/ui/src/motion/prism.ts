// Upright hexagonal pixel prism (9 × 15): a 1 px white beam drops into a symmetric
// 1-3-3-5-5-5-3-3-1 white core; the rim runs red → orange → yellow → green → blue → indigo → violet clockwise.
const OUTER = [1, 3, 5, 7, 9, 9, 9, 9, 9, 9, 9, 7, 5, 3, 1];
const CORE: Record<number, number> = { 3: 1, 4: 3, 5: 3, 6: 5, 7: 5, 8: 5, 9: 3, 10: 3, 11: 1 };
const RIM = ['#e5534b', '#f0874e', '#f2c94c', '#3fc085', '#4f8bff', '#5b5fe0', '#9b72cb'];

export interface Pixel { x: number; y: number; color: string }

export function prismPixels(): Pixel[] {
  const out: Pixel[] = [];
  const cx = 4, cy = 7;
  OUTER.forEach((width, y) => {
    const start = cx - (width - 1) / 2;
    const core = CORE[y] ?? 0;
    for (let x = start; x < start + width; x++) {
      const inCore = core > 0 && Math.abs(x - cx) <= (core - 1) / 2;
      const beam = x === cx && y < 3;
      if (inCore || beam) { out.push({ x, y, color: '#ffffff' }); continue; }
      // Clockwise angle from 12 o'clock, mapped onto the seven rim colours.
      const angle = (Math.atan2(x - cx, -(y - cy) * 0.62) + Math.PI * 2) % (Math.PI * 2);
      const color = RIM[Math.min(RIM.length - 1, Math.floor(angle / (Math.PI * 2) * RIM.length))] ?? '#ffffff';
      out.push({ x, y, color });
    }
  });
  return out;
}

export function prismSvg(): SVGSVGElement {
  const ns = 'http://www.w3.org/2000/svg';
  const svg = document.createElementNS(ns, 'svg');
  svg.setAttribute('viewBox', '0 0 9 15');
  svg.setAttribute('aria-hidden', 'true');
  svg.setAttribute('shape-rendering', 'crispEdges');
  for (const p of prismPixels()) {
    const r = document.createElementNS(ns, 'rect');
    r.setAttribute('x', String(p.x)); r.setAttribute('y', String(p.y));
    r.setAttribute('width', '1.02'); r.setAttribute('height', '1.02');
    r.setAttribute('fill', p.color);
    svg.append(r);
  }
  return svg;
}
