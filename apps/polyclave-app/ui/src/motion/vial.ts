import type { FrameClock } from './scheduler.js';
export function mountVial(canvas:HTMLCanvasElement,clock:FrameClock):void{
  const ctx=canvas.getContext('2d');if(!ctx)return;
  const n=24, heights=new Float32Array(n), velocity=new Float32Array(n), next=new Float32Array(n);
  let fraction=0,until=0;
  function render():void{
    if(!ctx)return;const width=canvas.width,height=canvas.height;
    ctx.clearRect(0,0,width,height);
    ctx.save();ctx.beginPath();ctx.roundRect(1,1,width-2,height-2,height*.34);ctx.clip();
    ctx.fillStyle='#8bbbe825';ctx.fillRect(0,0,width,height);
    const bottom=height-2,y=bottom-fraction*(height-4);
    const gradient=ctx.createLinearGradient(0,0,width,height);
    const color=fraction>=.91?'#f3a268':fraction>.58?'#58d7db':'#6bacf7';
    gradient.addColorStop(0,color);gradient.addColorStop(1,color+'77');
    ctx.beginPath();ctx.moveTo(0,height);
    for(let i=0;i<n;i++)ctx.lineTo(i/(n-1)*width,y+(heights[i]??0));
    ctx.lineTo(width,height);ctx.closePath();ctx.fillStyle=gradient;ctx.fill();ctx.restore();
    ctx.beginPath();ctx.roundRect(1,1,width-2,height-2,height*.34);
    ctx.strokeStyle='#a3c3e488';ctx.lineWidth=1;ctx.stroke();
  }
  function replenish(event:Event):void{
    if(!(event instanceof CustomEvent)||typeof event.detail!=='number')return;
    fraction=Math.max(0,Math.min(1,event.detail));
    velocity[10]=-1.5;velocity[13]=1;until=performance.now()+1100;
    if(clock.disabled)render();else clock.interact();
  }
  document.addEventListener('pc-context',replenish);
  canvas.width=canvas.id==='settings-vial'?320:104;canvas.height=canvas.id==='settings-vial'?54:28;
  render();
  clock.subscribe((t,dt)=>{
    if(t>until)return;
    const step=Math.min(dt*30,1);
    for(let i=1;i<n-1;i++){
      const h=heights[i]??0;
      velocity[i]=((velocity[i]??0)+(((heights[i-1]??0)+(heights[i+1]??0)-2*h)*.16-h*.035)*step)*.88;
      next[i]=h+(velocity[i]??0)*step;
    }
    heights.set(next);render();
  },()=>{heights.fill(0);velocity.fill(0);render();},()=>canvas.isConnected&&performance.now()<until);
}
