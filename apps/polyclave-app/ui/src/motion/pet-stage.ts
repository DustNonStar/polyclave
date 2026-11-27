import type { FrameClock } from './scheduler.js';

// Four original 16 × 10 pixel partners. No black outlines; eyes are 1 × 2 vertical pixels.
// Key: B body, S shade, L legs, E eye, T tail/fluke, A accent, N nose, W wing.
type PetState = 'idle' | 'thinking' | 'running' | 'review';
interface Pet { name: string; x: number; art: readonly string[]; palette: Record<string, string>; legs: boolean }

const DEEPY: readonly string[] = [
  'T..T............',
  'TTTT.....BBBBB..',
  '.TT....BBBBBBBB.',
  '..TT..BBBBBBBBBB',
  '...TBBBBBBBBBEBB',
  '....BBBBBBBBBEBB',
  '....BBBBBBSBBBBB',
  '.....LLLLLLLLLBB',
  '......LLLLLLLLB.',
  '................',
];
const DRAGON: readonly string[] = [
  '..A..........A..',
  '..AS........SA..',
  '..BBBBBBBBBBBB..',
  'W.BBBEBBBBEBBB.W',
  'WWBBBEBBBBEBBBWW',
  '.WBBBBBBBBBBBBW.',
  '..BBBSSSSSSBBB..',
  '..BBBBBBBBBBBB..',
  '...L.L....L.L...',
  '...L.L....L.L...',
];
const CLAWD: readonly string[] = [
  '.......A........',
  '.......A........',
  '..BBBBBBBBBBBB..',
  '..BBBEBBBBEBBB..',
  '..BBBEBBBBEBBB..',
  'BBBBBBBBBBBBBBBB',
  '..BBBBBBBBBBBB..',
  '..BBBSSSSSSBBB..',
  '...L.L....L.L...',
  '...L.L....L.L...',
];
const CAT: readonly string[] = [
  '..B..........B..',
  '..BB........BB..',
  '..BBBBBBBBBBBB..',
  '..BBBEBBBBEBBB.T',
  '..BBBEBNNBEBBB.T',
  '..BBBBBBBBBBBBTT',
  '..BBBBBBBBBBBB..',
  '..BBBSSSSSSBBB..',
  '...L.L....L.L...',
  '...L.L....L.L...',
];

const PETS: Pet[] = [
  { name: 'Deepy', x: 0, art: DEEPY, legs: false, palette: { B: '#3d6cf0', S: '#2f56c8', L: '#9db8ff', T: '#2f56c8', E: '#0c1638' } },
  { name: 'Dragon', x: 22, art: DRAGON, legs: true, palette: { B: '#e3e6ee', S: '#b4bac8', L: '#c8cdd8', A: '#9aa2b4', W: '#aab2c4', E: '#1a1d27' } },
  { name: 'Clawd', x: 44, art: CLAWD, legs: true, palette: { B: '#d97757', S: '#b85f43', L: '#c36a4d', A: '#f2a587', E: '#2a120a' } },
  { name: 'Cat', x: 66, art: CAT, legs: true, palette: { B: '#9b87f0', S: '#7d69d6', L: '#8a76e2', T: '#7d69d6', N: '#f4a7c1', E: '#1b1433' } },
];
export const STAGE_W = 88;
export const STAGE_H = 20;
const BASE_Y = 10;
const SPOUT = '#5cd6ff';

export function mountPets(canvas: HTMLCanvasElement, clock: FrameClock, caption?: HTMLElement | null): void {
  canvas.width = STAGE_W; canvas.height = STAGE_H;
  const ctx = canvas.getContext('2d');
  if (!ctx) return;
  let state: PetState = 'idle';
  let since = 0;

  const px = (x: number, y: number, color: string): void => { ctx.fillStyle = color; ctx.fillRect(x, y, 1, 1); };

  function drawPet(pet: Pet, left: number, top: number, t: number, opts: { sleep: boolean; walk: number; look: number }): void {
    for (let row = 0; row < pet.art.length; row++) {
      const line = pet.art[row] ?? '';
      for (let col = 0; col < line.length; col++) {
        const key = line[col] ?? '.';
        if (key === '.') continue;
        if (key === 'E') {
          // Sleeping: closed eyes are the lower pixel only.
          if (opts.sleep && row === 3 + (pet.name === 'Deepy' ? 1 : 0)) continue;
          px(left + col + opts.look, top + row, pet.palette.E ?? '#111');
          continue;
        }
        if (key === 'L' && pet.legs && opts.walk) {
          // Alternate leg pairs while walking.
          const outer = col === 3 || col === 12;
          if ((opts.walk > 0) === outer && row === 9) continue;
        }
        px(left + col, top + row, pet.palette[key] ?? pet.palette.B ?? '#fff');
      }
    }
    void t;
  }

  function draw(t: number): void {
    ctx!.clearRect(0, 0, STAGE_W, STAGE_H);
    const still = clock.disabled || t === 0;
    const secs = (t - since) / 1000;
    const sleep = state === 'idle';
    PETS.forEach((pet, i) => {
      let left = pet.x;
      let top = BASE_Y;
      let walk = 0;
      let look = 0;
      if (!still) {
        if (state === 'running') {
          top -= Math.round(Math.max(0, Math.sin(secs * 7 + i * 1.3)) * 1.6);
          walk = Math.sin(secs * 9 + i) > 0 ? 1 : -1;
        } else if (state === 'thinking') {
          top -= Math.round(Math.max(0, Math.sin(secs * 3 + i * 0.9)));
        } else if (state === 'review') {
          top -= i === 2 ? Math.round(Math.max(0, Math.sin(secs * 6)) * 2) : 0;
        }
        if (pet.name === 'Deepy') top += Math.round(Math.sin(secs * 2.2) * 0.6);
      }
      if (state === 'review' && i !== 2) look = i < 2 ? 1 : -1;
      // Stacking game while thinking: the cat hops onto Clawd every other beat.
      if (state === 'thinking' && i === 3 && !still && Math.floor(secs / 2.4) % 2 === 1) { left = 44; top = BASE_Y - 8; }
      drawPet(pet, left, top, t, { sleep, walk, look });
      if (pet.name === 'Deepy') drawSpout(left, top, secs, still);
    });
    if (state === 'review') {
      const x = 44 + 7, y = BASE_Y - 5;
      for (const [dx, dy] of [[0, 0], [0, 1], [0, 3]] as const) px(x + dx, y + dy, '#f09b78');
    }
    if (sleep) {
      // A small pixel "z" over the cat.
      const zx = 82, zy = 2;
      const z = ['###', '..#', '.#.', '###'];
      z.forEach((r, ry) => { for (let rx = 0; rx < r.length; rx++) if (r[rx] === '#') px(zx + rx, zy + ry, '#8fa3d9'); });
    }
    if (state === 'thinking' && !still) {
      const n = Math.floor(secs * 2.5) % 4;
      for (let d = 0; d < n; d++) px(22 + 9 + d * 2, BASE_Y - 3, '#6aaff6');
    }
  }

  function drawSpout(left: number, top: number, secs: number, still: boolean): void {
    const sx = left + 11;
    if (state === 'idle') return;
    if (still) { px(sx, top - 1, SPOUT); px(sx, top - 2, SPOUT); px(sx - 1, top - 3, SPOUT); px(sx + 1, top - 3, SPOUT); return; }
    const speed = state === 'running' ? 6 : 3.2;
    const phase = (secs * speed) % 4;
    const h = Math.floor(phase < 2.5 ? phase * 1.6 : (4 - phase) * 2.6);
    for (let k = 1; k <= Math.min(h, 4); k++) px(sx, top - k, SPOUT);
    if (h >= 3) {
      const fan = h - 2;
      px(sx - fan, top - h, SPOUT); px(sx + fan, top - h, SPOUT);
      px(sx - fan - 1, top - h + 1, '#a9ecff'); px(sx + fan + 1, top - h + 1, '#a9ecff');
    }
  }

  const captions: Record<PetState, string> = {
    idle: 'Partners resting',
    thinking: 'Thinking together',
    running: 'Workers in motion',
    review: 'Waiting for your review',
  };
  const change = (next: PetState): void => {
    state = next; since = performance.now();
    if (caption) caption.textContent = captions[next];
    if (clock.disabled) draw(0);
  };
  document.addEventListener('pc-start', () => change('thinking'));
  document.addEventListener('pc-stream', () => { if (state !== 'running') change('running'); });
  document.addEventListener('pc-idle', () => change('idle'));
  document.addEventListener('pc-review', () => change('review'));
  draw(0);
  clock.subscribe(t => draw(t), () => draw(0));
}
