import {FrameClock} from './scheduler.js';
import {mountAurora} from './aurora.js';
import {mountInk} from './ink-stream.js';
import {mountCaret} from './smooth-caret.js';
import {mountVial} from './vial.js';
import {mountPets} from './pet-stage.js';
import {addIcons} from './icons.js';
import {byId} from '../views/elements.js';
import type {Intensity} from './spring.js';

const clock=new FrameClock(document.documentElement);
const saved=localStorage.getItem('pc-motion');
if(saved==='off'||saved==='subtle'||saved==='standard'||saved==='ultra')clock.setIntensity(saved);
mountAurora(byId('aurora'),clock);
mountInk(byId('stream'),clock);
mountCaret(byId('prompt'),clock);
mountVial(byId('composer-vial'),clock);
mountPets(byId('pet-stage'),clock);
addIcons();
document.addEventListener('pc-start',()=>clock.setState('thinking'));
document.addEventListener('pc-stream',()=>clock.setState('running'));
document.addEventListener('pc-idle',()=>clock.setState('idle'));
document.addEventListener('pc-review',()=>clock.setState('review'));
document.addEventListener('pc-intensity',event=>{
 if(event instanceof CustomEvent)clock.setIntensity(event.detail as Intensity);
});
document.addEventListener('pc-settings',()=>{
 const vial=document.getElementById('settings-vial');
 if(vial instanceof HTMLCanvasElement)mountVial(vial,clock);
});
document.addEventListener('pointerdown',()=>clock.interact(),{passive:true});
clock.wake();
