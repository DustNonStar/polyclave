import type { FrameClock } from './scheduler.js';

// Four original 16 × 10 pixel partners. No black outlines; eyes are 1 × 2 vertical pixels.
// Key: B body, S shade, L legs, E eye, T tail/fluke, A accent, N nose, W wing.
type PetState = 'idle' | 'thinking' | 'running' | 'review';
type Behavior = 'sit' | 'walk' | 'nap' | 'hop';
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
  { name: 'Deepy', x: 4, art: DEEPY, legs: false, palette: { B: '#3d6cf0', S: '#2f56c8', L: '#9db8ff', T: '#2f56c8', E: '#0c1638' } },
  { name: 'Dragon', x: 34, art: DRAGON, legs: true, palette: { B: '#e3e6ee', S: '#b4bac8', L: '#c8cdd8', A: '#9aa2b4', W: '#aab2c4', E: '#1a1d27' } },
  { name: 'Clawd', x: 64, art: CLAWD, legs: true, palette: { B: '#d97757', S: '#b85f43', L: '#c36a4d', A: '#f2a587', E: '#2a120a' } },
  { name: 'Cat', x: 94, art: CAT, legs: true, palette: { B: '#9b87f0', S: '#7d69d6', L: '#8a76e2', T: '#7d69d6', N: '#f4a7c1', E: '#1b1433' } },
];

export const STAGE_W = 160;
export const STAGE_H = 20;
const SCALE = 3;
const PET_W = 16;
const PET_H = 10;
const BASE_Y = 10;
const SPOUT = '#5cd6ff';

interface PetRuntime {
  x: number;
  y: number;
  vx: number;
  vy: number;
  facing: 1 | -1;
  behavior: Behavior;
  nextSwitch: number;
  blinkUntil: number;
  nextBlink: number;
  spoutUntil: number;
  dragging: boolean;
}

export function mountPets(canvas: HTMLCanvasElement, clock: FrameClock, caption?: HTMLElement | null): void {
  canvas.width = STAGE_W * SCALE;
  canvas.height = STAGE_H * SCALE;
  const ctx = canvas.getContext('2d');
  if (!ctx) return;

  let state: PetState = 'idle';
  let since = performance.now();
  let lastT = 0;
  let lastPaint = 0;
  let raf = 0;
  let dragIdx = -1;
  let dragOffsetX = 0;
  let dragOffsetY = 0;
  let dragStartX = 0;
  let dragStartY = 0;
  let dragMoved = false;
  let hoverX: number | null = null;
  let cachedRect: DOMRect | null = null;

  const now = performance.now();
  const runtime: PetRuntime[] = PETS.map((p, i) => ({
    x: p.x,
    y: BASE_Y,
    vx: 0,
    vy: 0,
    facing: 1,
    behavior: i === 3 ? 'nap' : 'sit',
    nextSwitch: now + 1800 + i * 950,
    blinkUntil: 0,
    nextBlink: now + 2200 + i * 800,
    spoutUntil: 0,
    dragging: false,
  }));

  const clamp = (v: number, lo: number, hi: number): number => Math.max(lo, Math.min(hi, v));
  const px = (x: number, y: number, color: string): void => {
    ctx.fillStyle = color;
    ctx.fillRect(Math.round(x * SCALE), Math.round(y * SCALE), SCALE, SCALE);
  };

  function stageCoords(ev: PointerEvent): { x: number; y: number } {
    const r = cachedRect ?? canvas.getBoundingClientRect();
    return {
      x: ((ev.clientX - r.left) / Math.max(1, r.width)) * STAGE_W,
      y: ((ev.clientY - r.top) / Math.max(1, r.height)) * STAGE_H,
    };
  }

  function floorYFor(idx: number, x: number, y: number): number {
    let floor = BASE_Y;
    for (let j = 0; j < runtime.length; j++) {
      if (j === idx) continue;
      const other = runtime[j]!;
      if (other.dragging) continue;
      if (other.y > y + 4 && Math.abs(other.x - x) < 11) {
        // Cap stack at 2 high so pets stay neatly inside the dock.
        floor = Math.min(floor, Math.max(2, other.y - 8));
      }
    }
    return floor;
  }

  function schedule(): void {
    if (clock.disabled || document.hidden) {
      if (raf) { cancelAnimationFrame(raf); raf = 0; }
      lastT = 0;
      paint(0);
      return;
    }
    if (!raf) raf = requestAnimationFrame(tick);
  }

  function tick(t: number): void {
    raf = 0;
    if (clock.disabled || document.hidden) {
      lastT = 0;
      paint(0);
      return;
    }
    // Run at ~60fps while dragging/falling, ~30fps for relaxed ambient life.
    const interval = dragIdx >= 0 || runtime.some(r => r.vy !== 0 || Math.abs(r.vx) > 0.5) ? 14 : 32;
    if (lastPaint === 0 || t - lastPaint >= interval) {
      lastPaint = t;
      paint(t);
    }
    raf = requestAnimationFrame(tick);
  }

  canvas.addEventListener('pointerdown', (ev: PointerEvent) => {
    cachedRect = canvas.getBoundingClientRect();
    const pos = stageCoords(ev);
    for (let i = runtime.length - 1; i >= 0; i--) {
      const rt = runtime[i]!;
      if (pos.x >= rt.x - 2 && pos.x <= rt.x + PET_W + 2 && pos.y >= rt.y - 2 && pos.y <= rt.y + PET_H + 1) {
        dragIdx = i;
        dragOffsetX = pos.x - rt.x;
        dragOffsetY = pos.y - rt.y;
        dragStartX = pos.x;
        dragStartY = pos.y;
        dragMoved = false;
        rt.dragging = true;
        rt.behavior = 'sit';
        rt.vx = 0;
        rt.vy = 0;
        canvas.classList.add('is-dragging');
        try { canvas.setPointerCapture(ev.pointerId); } catch { /* ignore */ }
        paint(performance.now());
        schedule();
        break;
      }
    }
  });

  canvas.addEventListener('pointermove', (ev: PointerEvent) => {
    const pos = stageCoords(ev);
    hoverX = pos.x;
    if (dragIdx < 0) return;
    if (Math.hypot(pos.x - dragStartX, pos.y - dragStartY) > 1.2) dragMoved = true;
    const rt = runtime[dragIdx]!;
    const nx = clamp(pos.x - dragOffsetX, 0, STAGE_W - PET_W);
    const ny = clamp(pos.y - dragOffsetY, 0, BASE_Y);
    const dx = nx - rt.x;
    const dy = ny - rt.y;
    if (Math.abs(dx) > 0.25) rt.facing = dx > 0 ? 1 : -1;
    rt.vx = rt.vx * 0.45 + clamp(dx * 22, -75, 75) * 0.55;
    rt.vy = rt.vy * 0.45 + clamp(dy * 22, -55, 55) * 0.55;
    rt.x = nx;
    rt.y = ny;
    paint(performance.now());
  });

  canvas.addEventListener('pointerleave', () => { hoverX = null; });

  const releaseDrag = (ev: PointerEvent): void => {
    if (dragIdx < 0) return;
    const rt = runtime[dragIdx]!;
    const pet = PETS[dragIdx]!;
    rt.dragging = false;
    if (!dragMoved) {
      rt.vy = -28;
      rt.behavior = 'hop';
      rt.nextSwitch = performance.now() + 900;
      if (pet.name === 'Deepy') rt.spoutUntil = performance.now() + 1500;
    }
    dragIdx = -1;
    cachedRect = null;
    canvas.classList.remove('is-dragging');
    try { canvas.releasePointerCapture(ev.pointerId); } catch { /* ignore */ }
    schedule();
  };
  canvas.addEventListener('pointerup', releaseDrag);
  canvas.addEventListener('pointercancel', releaseDrag);

  function stepBehaviors(t: number, dt: number): void {
    for (let i = 0; i < runtime.length; i++) {
      const rt = runtime[i]!;
      const pet = PETS[i]!;
      if (rt.dragging) continue;
      if (t >= rt.nextBlink) {
        rt.blinkUntil = t + 140;
        rt.nextBlink = t + 2600 + Math.random() * 3400;
      }
      if (state === 'idle' && t >= rt.nextSwitch) {
        const roll = Math.random();
        if (roll < 0.44) {
          rt.behavior = 'walk';
          rt.facing = rt.x < 18 ? 1 : rt.x > STAGE_W - PET_W - 18 ? -1 : Math.random() < 0.5 ? 1 : -1;
          rt.nextSwitch = t + 1400 + Math.random() * 2000;
        } else if (roll < 0.72) {
          rt.behavior = 'sit';
          rt.nextSwitch = t + 2000 + Math.random() * 2600;
        } else if (roll < 0.88) {
          rt.behavior = 'nap';
          rt.nextSwitch = t + 3200 + Math.random() * 3000;
        } else {
          rt.behavior = 'hop';
          if (rt.y >= BASE_Y - 0.5) rt.vy = -20;
          if (pet.name === 'Deepy') rt.spoutUntil = t + 1100;
          rt.nextSwitch = t + 900;
        }
      }
      if (state === 'idle' && rt.behavior === 'walk' && rt.y >= BASE_Y - 0.5 && Math.abs(rt.vx) < 1) {
        const speed = pet.name === 'Deepy' ? 7 : 9;
        rt.x += rt.facing * speed * dt;
        if (rt.x <= 2) { rt.x = 2; rt.facing = 1; }
        if (rt.x >= STAGE_W - PET_W - 2) { rt.x = STAGE_W - PET_W - 2; rt.facing = -1; }
      }
    }
  }

  function stepPhysics(t: number): void {
    const dt = lastT ? Math.min((t - lastT) / 1000, 0.05) : 0.016;
    lastT = t;
    stepBehaviors(t, dt);
    for (let i = 0; i < runtime.length; i++) {
      const rt = runtime[i]!;
      if (rt.dragging) continue;
      for (let j = i + 1; j < runtime.length; j++) {
        const other = runtime[j]!;
        if (other.dragging) continue;
        const dx = rt.x - other.x;
        const dy = Math.abs(rt.y - other.y);
        if (dy < 4 && Math.abs(dx) < 13) {
          const dir = dx === 0 ? (i < j ? -1 : 1) : dx > 0 ? 1 : -1;
          const push = (13 - Math.abs(dx)) * 5 * dt * dir;
          rt.x = clamp(rt.x + push, 0, STAGE_W - PET_W);
          other.x = clamp(other.x - push, 0, STAGE_W - PET_W);
        }
      }
      const floor = floorYFor(i, rt.x, rt.y);
      if (rt.y < floor || rt.vy !== 0 || Math.abs(rt.vx) > 0.3) {
        rt.vy += 130 * dt;
        rt.x = clamp(rt.x + rt.vx * dt, 0, STAGE_W - PET_W);
        rt.y += rt.vy * dt;
        rt.vx *= 0.90;
        if (rt.x <= 0 || rt.x >= STAGE_W - PET_W) rt.vx = -rt.vx * 0.45;
        if (rt.y >= floor) {
          rt.y = floor;
          if (Math.abs(rt.vy) > 12) rt.vy = -rt.vy * 0.28;
          else rt.vy = 0;
        }
        if (Math.abs(rt.vx) <= 0.3) rt.vx = 0;
      } else if (rt.y > floor) {
        rt.y = floor;
      }
    }
  }

  function drawPet(pet: Pet, left: number, top: number, opts: { sleep: boolean; walk: number; look: number; flip: boolean }): void {
    for (let row = 0; row < pet.art.length; row++) {
      const line = pet.art[row] ?? '';
      for (let col = 0; col < line.length; col++) {
        const key = line[col] ?? '.';
        if (key === '.') continue;
        const drawCol = opts.flip ? PET_W - 1 - col : col;
        if (key === 'E') {
          if (opts.sleep && row === 3 + (pet.name === 'Deepy' ? 1 : 0)) continue;
          px(left + drawCol + opts.look, top + row, pet.palette.E ?? '#111');
          continue;
        }
        if (key === 'L' && pet.legs && opts.walk) {
          const outer = col === 3 || col === 12;
          if ((opts.walk > 0) === outer && row === 9) continue;
        }
        px(left + drawCol, top + row, pet.palette[key] ?? pet.palette.B ?? '#fff');
      }
    }
  }

  function paint(t: number): void {
    ctx!.clearRect(0, 0, STAGE_W * SCALE, STAGE_H * SCALE);
    const still = clock.disabled || t === 0;
    const secs = (t - since) / 1000;
    if (!still) stepPhysics(t);

    PETS.forEach((pet, i) => {
      const rt = runtime[i]!;
      let left = rt.x;
      let top = rt.y;
      let walk = 0;
      let look = 0;
      const sleeping = !still && !rt.dragging && state === 'idle' && rt.behavior === 'nap';
      const blinking = !still && t < rt.blinkUntil;
      if (!still) {
        if (rt.dragging) {
          walk = Math.sin(t * 0.018) > 0 ? 1 : -1;
        } else if (state === 'running') {
          top -= Math.max(0, Math.sin(secs * 7 + i * 1.3)) * 1.5;
          walk = Math.sin(secs * 9 + i) > 0 ? 1 : -1;
        } else if (state === 'thinking') {
          top -= Math.max(0, Math.sin(secs * 3 + i * 0.9));
        } else if (state === 'review') {
          top -= i === 2 ? Math.max(0, Math.sin(secs * 6)) * 2 : 0;
        } else if (rt.behavior === 'walk') {
          walk = Math.sin(t * 0.012 + i) > 0 ? 1 : -1;
        }
        if (pet.name === 'Deepy' && !rt.dragging) top += Math.sin(t * 0.0024) * 0.45;
      }
      if (state === 'review' && i !== 2) look = i < 2 ? 1 : -1;
      else if (hoverX !== null && !rt.dragging && !sleeping) {
        look = hoverX > left + PET_W ? 1 : hoverX < left ? -1 : 0;
      }
      if (state === 'thinking' && i === 3 && !still && !rt.dragging && Math.floor(secs / 2.4) % 2 === 1) {
        left = runtime[2]!.x;
        top = Math.max(2, runtime[2]!.y - 8);
      }
      const flip = pet.name === 'Deepy' ? rt.facing < 0 : false;
      drawPet(pet, left, top, { sleep: sleeping || blinking || still, walk, look, flip });
      if (pet.name === 'Deepy') drawSpout(left, top, t * 0.001, still, t < rt.spoutUntil, flip);
      if (sleeping) {
        const zx = Math.min(STAGE_W - 4, left + PET_W - 1);
        const zy = Math.max(1, top - 6 + Math.sin(t * 0.003 + i) * 0.8);
        const z = ['###', '..#', '.#.', '###'];
        z.forEach((r, ry) => { for (let rx = 0; rx < r.length; rx++) if (r[rx] === '#') px(zx + rx, zy + ry, '#8fa3d9'); });
      }
    });

    if (state === 'review') {
      const x = runtime[2]!.x + 7, y = runtime[2]!.y - 5;
      for (const [dx, dy] of [[0, 0], [0, 1], [0, 3]] as const) px(x + dx, y + dy, '#f09b78');
    }
    if (state === 'thinking' && !still) {
      const n = Math.floor(secs * 2.5) % 4;
      const dx = runtime[1]!.x + 9;
      for (let d = 0; d < n; d++) px(dx + d * 2, Math.max(1, runtime[1]!.y - 3), '#6aaff6');
    }
  }

  function drawSpout(left: number, top: number, secs: number, still: boolean, forced: boolean, flip: boolean): void {
    const sx = flip ? left + 4 : left + 11;
    if (state === 'idle' && !forced) return;
    if (still) { px(sx, top - 1, SPOUT); px(sx, top - 2, SPOUT); px(sx - 1, top - 3, SPOUT); px(sx + 1, top - 3, SPOUT); return; }
    const speed = state === 'running' || forced ? 6 : 3.2;
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
    state = next;
    since = performance.now();
    if (caption) caption.textContent = captions[next];
    schedule();
  };
  document.addEventListener('pc-start', () => change('thinking'));
  document.addEventListener('pc-stream', () => { if (state !== 'running') change('running'); });
  document.addEventListener('pc-idle', () => change('idle'));
  document.addEventListener('pc-review', () => change('review'));
  document.addEventListener('pc-intensity', () => schedule());
  document.addEventListener('visibilitychange', () => schedule());
  clock.reduced.addEventListener('change', () => schedule());
  paint(0);
  schedule();
}
