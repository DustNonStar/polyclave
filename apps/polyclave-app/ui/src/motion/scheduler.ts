import type { Intensity } from './spring.js';
export type FrameCallback=(t:number,seconds:number)=>void;
type Member={paint:FrameCallback;sleep?:(state:string)=>void;active:()=>boolean};
export class FrameClock {
  private raf=0;private last=0;private paintTime=0;private interacted=0;private since=0;
  private members=new Set<Member>();
  private mode:Intensity='standard';private state='idle';
  readonly reduced=matchMedia('(prefers-reduced-motion: reduce)');
  constructor(private root:HTMLElement){
    this.reduced.addEventListener('change',()=>this.wake());
    document.addEventListener('visibilitychange',()=>this.wake());
    this.root.dataset.motion='0fps';
  }
  get disabled():boolean{return this.reduced.matches||this.mode==='off';}
  setIntensity(value:Intensity):void{this.mode=value;this.wake();}
  subscribe(paint:FrameCallback,sleep?:(state:string)=>void,active:()=>boolean=()=>true):()=>void{
    const m={paint,sleep,active};this.members.add(m);
    return()=>{this.members.delete(m);};
  }
  setState(state:string):void{this.state=state;this.wake();}
  interact():void{this.interacted=performance.now()+300;this.wake();}
  wake():void{
    this.since=performance.now();
    if(this.disabled||document.hidden){this.stop();return;}
    if(!this.raf){this.last=0;this.paintTime=0;this.raf=requestAnimationFrame(this.tick);}
  }
  private stop():void{
    if(this.raf)cancelAnimationFrame(this.raf);
    this.raf=0;this.root.dataset.motion='0fps';this.root.dataset.motionState=this.state;
    for(const m of this.members)m.sleep?.(this.state);
  }
  private tick=(t:number):void=>{
    this.raf=0;
    if(this.disabled||document.hidden){this.stop();return;}
    if((this.state==='idle'||this.state==='review')&&t-this.since>=1200){this.stop();return;}
    const burst=t<this.interacted;
    const limit=burst?(this.mode==='subtle'?16:7):(this.mode==='subtle'?48:1000/30);
    this.root.dataset.motion=burst?'interactive':'30fps';
    this.root.dataset.motionState=this.state;
    if(this.paintTime===0||t-this.paintTime>=limit-.4){
      const dt=this.last?Math.min((t-this.last)/1000,.05):.016;
      this.paintTime=t;this.last=t;
      for(const m of this.members)if(m.active())m.paint(t,dt);
    }
    this.raf=requestAnimationFrame(this.tick);
  };
}
