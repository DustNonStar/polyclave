export function byId<T extends HTMLElement=HTMLElement>(id:string):T{
  const node=document.getElementById(id);if(!node)throw Error('Missing '+id);return node as T;
}
export function el<K extends keyof HTMLElementTagNameMap>(tag:K,text?:string,cls?:string):HTMLElementTagNameMap[K]{
  const node=document.createElement(tag);if(text!==undefined)node.textContent=text;if(cls)node.className=cls;return node;
}
export function button(text:string,action:()=>void,cls?:string):HTMLButtonElement{
  const b=el('button',text,cls);b.type='button';b.addEventListener('click',action);return b;
}
export function card(title:string,content?:string,state?:string):HTMLElement{
  const item=el('section',undefined,'card');item.append(el('h3',title));
  if(content)item.append(el('p',content,'section-detail'));if(state)item.dataset.state=state;return item;
}
