import type { FrameClock } from './scheduler.js';
import { position } from './spring.js';
export function mountCaret(input:HTMLTextAreaElement,clock:FrameClock):void{
  const parent=input.parentElement;if(!parent)return;
  const mirror=document.createElement('div');mirror.className='caret-measure';mirror.setAttribute('aria-hidden','true');
  const before=document.createTextNode('');const point=document.createElement('span');point.textContent='​';mirror.append(before,point);
  const caret=document.createElement('span');caret.className='optical-caret';caret.setAttribute('aria-hidden','true');
  parent.append(mirror,caret);
  let dirty=false,active=false,readAt=false,x=0,y=0,fromX=0,fromY=0,toX=0,toY=0,at=0;
  const queue=()=>{dirty=true;clock.interact();};
  input.addEventListener('focus',()=>{active=true;queue();});
  input.addEventListener('blur',()=>{active=false;caret.classList.remove('on');input.classList.remove('custom-caret');});
  for(const name of ['input','click','keyup','select','scroll'] as const)input.addEventListener(name,queue);
  document.addEventListener('selectionchange',()=>{if(active)queue();});
  clock.subscribe(time=>{
    if(!active||clock.disabled||input.selectionStart!==input.selectionEnd){
      caret.classList.remove('on');input.classList.remove('custom-caret');return;
    }
    if(readAt){
      const pos=point.getBoundingClientRect(),frame=mirror.getBoundingClientRect();
      fromX=x;fromY=y;toX=pos.left-frame.left-input.scrollLeft;toY=pos.top-frame.top-input.scrollTop;at=time;readAt=false;
    }
    if(dirty){
      const style=getComputedStyle(input);
      mirror.style.width=input.getBoundingClientRect().width+'px';
      for(const p of ['font','line-height','letter-spacing','padding','border','tab-size'])mirror.style.setProperty(p,style.getPropertyValue(p));
      before.data=input.value.slice(0,input.selectionStart);dirty=false;readAt=true;
    }
    const f=position('snappy',Math.min(.55,(time-at)/1000));
    x=fromX+(toX-fromX)*f;y=fromY+(toY-fromY)*f;
    if(time>at){caret.style.transform='translate('+x+'px,'+y+'px)';caret.classList.add('on');input.classList.add('custom-caret');}
  },()=>{caret.classList.remove('on');input.classList.remove('custom-caret');},()=>active);
}
