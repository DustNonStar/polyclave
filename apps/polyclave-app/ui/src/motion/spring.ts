export const springs={
  snappy:{k:420,c:30,m:.9},bouncy:{k:380,c:20,m:1},
  default:{k:380,c:26,m:1},gentle:{k:260,c:26,m:1.05},
} as const;
export type Preset=keyof typeof springs;
export type Intensity='off'|'subtle'|'standard'|'ultra';
export function position(preset:Preset,t:number):number{
  const {k,c,m}=springs[preset];
  const decay=c/(2*m),omega=Math.sqrt(k/m);
  const damped=Math.sqrt(Math.max(0,omega*omega-decay*decay));
  if(damped<.00001)return 1-Math.exp(-decay*t)*(1+decay*t);
  return 1-Math.exp(-decay*t)*(Math.cos(damped*t)+decay/damped*Math.sin(damped*t));
}
export function easing(preset:Preset,s=.63):string{
  return 'linear('+Array.from({length:40},(_,i)=>i===39?'1':position(preset,s*i/39).toFixed(5)).join(', ')+')';
}
export function play(node:HTMLElement,frames:Keyframe[],preset:Preset,duration:number):void{
  if(matchMedia('(prefers-reduced-motion: reduce)').matches||!node.animate)return;
  const curve=easing(preset);node.animate(frames,{duration,easing:CSS.supports('animation-timing-function',curve)?curve:'ease-out'});
}
