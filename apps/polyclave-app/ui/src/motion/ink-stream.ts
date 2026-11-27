import type { FrameClock } from './scheduler.js';
interface Ranges { add(r:Range):void }
const ids=['pc-ink-1','pc-ink-2','pc-ink-3','pc-ink-4','pc-ink-5'];
export function mountInk(target:HTMLElement,clock:FrameClock):void{
  const css=CSS as typeof CSS & {highlights?:Map<string,Ranges>};
  const ctor=(globalThis as unknown as {Highlight?:new()=>Ranges}).Highlight;
  if(!css.highlights||!ctor)return;
  const registry=css.highlights,Highlight=ctor;
  const spans:{from:number;to:number;at:number}[]=[];
  let last=0;
  function clear():void{for(const id of ids)registry.delete(id);spans.length=0;}
  document.addEventListener('pc-stream',()=>{
    const length=target.textContent?.length??0;
    if(length<last)clear();
    if(!clock.disabled&&length>last)spans.push({from:last,to:length,at:performance.now()});
    last=length;clock.wake();
  });
  document.addEventListener('pc-idle',()=>{last=0;clear();});
  clock.subscribe(time=>{
    if(!spans.length)return;
    const leaf=target.firstChild;
    if(!(leaf instanceof Text)){clear();return;}
    const buckets=ids.map(()=>new Highlight());
    for(let i=spans.length-1;i>=0;i--){
      const entry=spans[i];if(!entry)continue;
      const tier=Math.floor((time-entry.at)/36);
      if(tier>=5||entry.to>leaf.length){spans.splice(i,1);continue;}
      const r=new Range();r.setStart(leaf,entry.from);r.setEnd(leaf,entry.to);buckets[Math.max(0,tier)]?.add(r);
    }
    if(!spans.length){clear();return;}
    for(let i=0;i<buckets.length;i++){const id=ids[i], bucket=buckets[i];if(id&&bucket)registry.set(id,bucket);}
  },clear,()=>spans.length>0);
}
