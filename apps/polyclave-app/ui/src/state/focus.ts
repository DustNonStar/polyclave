export function withFocusPreserved(root:HTMLElement,render:()=>void):void{
  const active=document.activeElement instanceof HTMLElement?document.activeElement:null;
  const key=active&&root.contains(active)?active.dataset.focusKey:undefined;
  const selection=active instanceof HTMLInputElement||active instanceof HTMLTextAreaElement
    ?[active.selectionStart,active.selectionEnd] as const:null;
  render();
  if(!key||active?.isConnected)return;
  const next=Array.from(root.querySelectorAll<HTMLElement>('[data-focus-key]')).find(el=>el.dataset.focusKey===key);
  next?.focus({preventScroll:true});
  if(selection && selection[0]!==null && selection[1]!==null &&
      (next instanceof HTMLInputElement||next instanceof HTMLTextAreaElement)){
    try{next.setSelectionRange(selection[0],selection[1]);}catch{/* no text selection for this input type */}
  }
}
