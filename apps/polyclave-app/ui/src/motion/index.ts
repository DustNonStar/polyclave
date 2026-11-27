import { FrameClock } from './scheduler.js';
import { mountAurora } from './aurora.js';
import { mountInk } from './ink-stream.js';
import { mountCaret } from './smooth-caret.js';
import { mountVial } from './vial.js';
import { mountPets } from './pet-stage.js';
import { prismSvg } from './prism.js';
import { byId } from '../views/elements.js';
import type { Intensity } from './spring.js';

export const clock = new FrameClock(document.documentElement);
let saved: string | null = null;
try { saved = localStorage.getItem('pc-motion'); } catch { /* storage unavailable */ }
if (saved === 'off' || saved === 'subtle' || saved === 'standard' || saved === 'ultra') clock.setIntensity(saved);

byId('brand-mark').append(prismSvg());
const prism = byId('composer-prism');
prism.append(prismSvg());
for (const cls of ['a', 'b']) { const s = document.createElement('i'); s.className = 'spark ' + cls; prism.append(s); }

mountAurora(byId<HTMLCanvasElement>('aurora'), clock);
mountInk(byId('stream'), clock);
mountCaret(byId<HTMLTextAreaElement>('prompt'), clock);
mountVial(byId<HTMLCanvasElement>('composer-vial'), clock);
mountPets(byId<HTMLCanvasElement>('pet-stage'), clock, document.getElementById('pet-caption'));

document.addEventListener('pc-start', () => clock.setState('thinking'));
document.addEventListener('pc-stream', () => clock.setState('running'));
document.addEventListener('pc-idle', () => clock.setState('idle'));
document.addEventListener('pc-review', () => clock.setState('review'));
document.addEventListener('pc-intensity', event => {
  if (event instanceof CustomEvent) clock.setIntensity(event.detail as Intensity);
});
document.addEventListener('pc-settings', () => {
  const vial = document.getElementById('settings-vial');
  if (vial instanceof HTMLCanvasElement) mountVial(vial, clock);
});
document.addEventListener('pointerdown', () => clock.interact(), { passive: true });
clock.wake();
