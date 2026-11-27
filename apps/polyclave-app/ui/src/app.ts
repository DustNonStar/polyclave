import './motion/index.js';
import { bootstrap,follow,native,request } from './api/sse.js';
import type { Connection,Frame } from './api/sse.js';
import type { MemoryItemDto,MessageDto,RunDto,SessionDto } from './generated/proto.js';
import { byId,button,card,el } from './views/elements.js';
import { withFocusPreserved } from './state/focus.js';

type Saved=Pick<MessageDto,'id'|'source'|'text'|'queued'>;
type Profile={id:string;display_name:string;models:string[];kind:string};
type Review={run:RunDto;commit:{files:string;commits:string}|null};
type ProviderUsage={vendor:string;five_hour?:number;weekly?:number;cost?:number;context?:number;balance?:number};
type Inspector='Topology'|'Review'|'Files'|'Memory';
const dialog=byId<HTMLDialogElement>('modal');
const content=byId('dialog-content');
const root=byId('app');
const current=new AbortController();
let connection:Connection|null=null;
let sessions:SessionDto[]=[];let runs:RunDto[]=[];let saved:Saved[]=[];let profiles:Profile[]=[];let usage:ProviderUsage[]=[];
let selected:string|null=null;let activeRun:string|null=null;let panel:Inspector='Topology';
let live='';let pendingSync=false;
const host=<T>(path:string,body?:unknown)=>connection?request<T>(connection,path,body):Promise.reject(Error('Connect to the local daemon first'));
const sessionPath=()=>'/api/sessions/'+encodeURIComponent(selected??'');
const tell=(message:string)=>{byId('announcement').textContent=message;byId('connection').textContent=message;};
const problem=(e:unknown)=>tell(e instanceof Error?e.message:'Local operation did not finish');
const runById=()=>runs.find(v=>v.id===activeRun)??runs.find(v=>v.hub_session_id===selected);
function open(title:string,...nodes:Node[]):void{content.replaceChildren(el('h2',title),...nodes,button('Close',()=>dialog.close()));if(!dialog.open)dialog.showModal();}
function field(label:string,value='',type='text'):HTMLLabelElement {
  const l=el('label',label);const input=el('input');input.value=value;input.type=type;l.append(input);return l;
}
function input(label:HTMLLabelElement):HTMLInputElement{return label.querySelector('input')!;}
function theme(light:boolean):void{
  document.documentElement.dataset.theme=light?'light':'dark';
  localStorage.setItem('pc-theme',light?'light':'dark');
  byId('theme').textContent=light?'Dark theme':'Light theme';
}
theme(localStorage.getItem('pc-theme')==='light');
byId('theme').addEventListener('click',()=>theme(document.documentElement.dataset.theme!=='light'));
function pane(value:string):void{
  root.dataset.pane=value;
  for(const b of root.querySelectorAll('.mobile-nav button'))b.setAttribute('aria-pressed',String((b as HTMLElement).dataset.pane===value));
}
for(const b of root.querySelectorAll<HTMLButtonElement>('.mobile-nav button'))b.addEventListener('click',()=>pane(b.dataset.pane??'chat'));
function focusKey(node:HTMLElement,id:string):void{node.dataset.focusKey=id;}
function updateSidebar():void{
  const s=byId('sessions');
  withFocusPreserved(s,()=>{
    s.replaceChildren();
    if(!sessions.length)s.append(el('p','No registered projects yet. Use + to add a local repository.','section-detail'));
    for(const item of sessions.filter(x=>x.role!=='worker')){
      const b=button(item.name,()=>{selected=item.id;live='';activeRun=null;pane('chat');void sync().catch(problem);},'task-nav');
      b.title=item.repo_cwd;b.setAttribute('aria-pressed',String(item.id===selected));focusKey(b,item.id);s.append(b);
    }
  });
  const r=byId('worker-runs');
  withFocusPreserved(r,()=>{
    r.replaceChildren();
    for(const item of runs.slice().reverse().slice(0,30)){
      const b=button(item.id+' · '+item.status.replaceAll('_',' '),()=>{activeRun=item.id;panel='Review';pane('inspect');void renderInspector().catch(problem);},'task-nav');
      b.setAttribute('aria-pressed',String(item.id===activeRun));focusKey(b,item.id);r.append(b);
    }
  });
}
function renderChat():void{
  const s=byId('history');
  const name=sessions.find(v=>v.id===selected);
  byId('title').textContent=name?.name??'Workspace Hub';
  const select=byId<HTMLSelectElement>('provider');
  const model=name?name.profile_id+'|'+name.provider_model:'';
  if(!select.matches(':focus')){
    select.replaceChildren(new Option('Choose model',''));
    for(const p of profiles)for(const m of p.models)select.add(new Option(p.display_name+(m==='default'?'':' · '+m),p.id+'|'+m));
    select.value=model;
  }
  const scrolling=byId('history-scroll');
  const atBottom=scrolling.scrollHeight-scrolling.scrollTop-scrolling.clientHeight<56;
  withFocusPreserved(s,()=>{
    s.replaceChildren();
    if(!saved.length)s.append(card('A local place to coordinate your work','Register a project, choose a provider, and send the Hub a message.'));
    for(const m of saved){
      const item=el('article',undefined,'card chat-card');item.dataset.kind=m.source;
      const top=el('footer');
      top.append(el('small',m.source==='user'?(m.queued?'YOU · QUEUED':'YOU'):m.source==='agent'?'HUB':'HOST'));
      if(m.source==='user'){
        const b=button('Remember this rule',()=>draft(m.text,m.id));focusKey(b,'memory-'+m.id);top.append(b);
      }
      item.append(top,el('p',m.text));s.append(item);
    }
  });
  if(atBottom)scrolling.scrollTop=scrolling.scrollHeight;
}
async function sync():Promise<void>{
  if(!connection)return;
  [sessions,runs,profiles]=await Promise.all([host<SessionDto[]>('/api/sessions'),host<RunDto[]>('/api/runs'),host<Profile[]>('/api/profiles')]);
  selected??=sessions.find(s=>s.role==='hub')?.id??null;
  saved=selected?await host<Saved[]>(sessionPath()+'/messages'):[];
  updateSidebar();renderChat();await renderInspector();renderQuotas();
}
function queueSync():void{
  if(pendingSync)return;pendingSync=true;
  setTimeout(()=>{pendingSync=false;void sync().catch(problem);},50);
}
async function frame(f:Frame):Promise<void>{
  if(f.event==='resync_required'){await sync();return;}
  if(f.event==='change'){queueSync();return;}
  if(f.event==='agent'){
    const e=f.data as {run_id:string;message:{kind:string;text?:string}};
    if(e.run_id!==selected)return;
    if(e.message.kind==='text_delta'&&e.message.text){live+=e.message.text;byId('stream').textContent=live;document.dispatchEvent(new Event('pc-stream'));}
    if(e.message.kind==='finished'){live='';byId('stream').textContent='';queueSync();document.dispatchEvent(new Event('pc-idle'));}
  }
}
async function renderInspector():Promise<void>{
  const tabs=byId('inspect-tabs');
  if(!tabs.childElementCount){
    for(const name of ['Topology','Review','Files','Memory'] as const){
      const b=button(name,()=>{panel=name;void renderInspector().catch(problem);});b.id='tab-'+name.toLowerCase();b.setAttribute('role','tab');tabs.append(b);
    }
  }
  for(const b of tabs.children)b.setAttribute('aria-selected',String(b.textContent===panel));
  const which=panel;
  const p=byId('inspect');
  p.setAttribute('aria-labelledby','tab-'+which.toLowerCase());
  const list=el('div',undefined,'records');
  const run=runById();
  if(which==='Topology'){
    list.append(card('Hub · Coordination','Host-owned ledger and task receipts','running'));
    for(const r of runs.filter(v=>v.hub_session_id===selected)){
      const item=card(r.vendor+' · '+r.id,r.instructions.slice(0,180),r.status);
      item.append(button('Review '+r.id,()=>{activeRun=r.id;panel='Review';void renderInspector().catch(problem);}));
      list.append(item);
    }
  }else if(which==='Review'&&run){
    const result=await host<Review>('/api/runs/'+run.id).catch(()=>null);
    if(panel!==which)return;
    const item=card(run.status.replaceAll('_',' '),run.detail??'Host verification and decision history',run.status);
    if(result?.commit){
      item.append(el('pre',result.commit.commits+'\n'+result.commit.files,'file-output'));
    }
    if(run.status==='needs_review'||run.status==='interrupted'){
      const actions=el('div',undefined,'actions');
      const sha=run.observed_sha??run.result_sha;
      if(sha)actions.append(button('Accept '+sha.slice(0,8),()=>resolveRun(run.id,'accept',sha),'primary'));
      actions.append(button('Mark failed',()=>resolveRun(run.id,'fail')),button('Allow retry',()=>resolveRun(run.id,'allow_retry')));
      item.append(actions);
    }
    list.append(item);
  }else if(which==='Files'){
    const item=card('Project files','Read-only access through the registered repository.');
    const path=field('Relative file path');const output=el('pre','Enter a file path','file-output');
    item.append(path,button('Read file',()=>{
      void host<{text:string}>(sessionPath()+'/repo-file',{path:input(path).value}).then(v=>{output.textContent=v.text;}).catch(problem);
    }),output);list.append(item);
  }else if(which==='Memory'){
    list.append(card('Verified memory','Rules carry across providers. Drafts become active only after you confirm.'));
    if(selected&&connection){
      const result=await host<MemoryItemDto[]>(sessionPath()+'/memory').catch(()=>[]);
      if(panel!==which)return;
      for(const m of result){const c=card(m.title,m.content,m.status);c.append(el('small',m.kind+' · '+m.scope));list.append(c);}
    }
  }else list.append(card('No Worker run selected','Start a task through the Hub to see host verification here.'));
  if(panel===which)withFocusPreserved(p,()=>p.replaceChildren(list));
}
function draft(text:string,message?:number,run?:string):void{
  const fieldTitle=field('Rule title','Project correction');const rule=el('textarea');rule.value=text;
  const label=el('label','Rule text');label.append(rule);
  const kind=el('select');for(const k of ['constraint','preference','pitfall'])kind.add(new Option(k,k));
  open('Remember this rule',el('p','Review the wording before saving. Nothing is applied until you confirm.'),fieldTitle,label,kind,
    button('Save verified rule',()=>{void(async()=>{
      const value=await host<MemoryItemDto>(sessionPath()+'/memory',{action:run?'draft_run':'draft',message_id:message,run_id:run,kind:kind.value,title:input(fieldTitle).value,content:rule.value});
      await host(sessionPath()+'/memory',{action:'commit',id:value.id});dialog.close();panel='Memory';await renderInspector();tell('Verified rule saved');
    })().catch(problem);},'primary'));
}
function resolveRun(id:string,decision:string,sha?:string):void{
  const note=field('Decision note');
  open('Review decision',note,button('Confirm',()=>{void(async()=>{
    const value=input(note).value.trim();
    await host('/api/runs/'+id+'/resolve',{decision,seen_sha:sha,note:value});
    dialog.close();await sync();
    if(decision!=='accept'&&value)draft(value,undefined,id);
  })().catch(problem);},'primary'));
}
const vendorNames=['Claude','Codex','Gemini','DeepSeek'];
function renderQuotas():void{
  const node=byId('quota');
  node.replaceChildren();
  for(const vendor of vendorNames){
    const data=usage.find(v=>v.vendor.toLowerCase()===vendor.toLowerCase());
    const ring=el('span',undefined,'quota-ring');ring.dataset.vendor=vendor.toLowerCase();
    ring.title=data?(vendor+' · 5 hr '+(data.five_hour??'unknown')+' · weekly '+(data.weekly??'unknown')):vendor+' · quota data unavailable';
    ring.setAttribute('aria-label',ring.title);ring.append(el('span',vendor.slice(0,1)));node.append(ring);
  }
}
function settings():void{
  const body=el('div',undefined,'modal-grid');
  body.append(card('Provider limits','5-hour and weekly limits appear only when the host has verified local or provider data.'));
  for(const v of vendorNames){
    const u=usage.find(item=>item.vendor.toLowerCase()===v.toLowerCase());
    body.append(card(v,u?('Balance: '+(u.balance??'unavailable')+' · cost: '+(u.cost??'unavailable')):'Usage unavailable'));
  }
  const usageRow=card('512K Context','Context capacity is displayed separately from provider quota.');
  const canvas=el('canvas');canvas.id='settings-vial';usageRow.append(canvas);
  const chart=el('div','No recorded cost data','file-output');chart.id='cost-ledger';usageRow.append(chart);
  body.append(usageRow);
  const motion=el('label','Motion');const intensity=el('select');
  for(const value of ['off','subtle','standard','ultra'])intensity.add(new Option(value,value));
  intensity.value=localStorage.getItem('pc-motion')??'standard';
  intensity.addEventListener('change',()=>{localStorage.setItem('pc-motion',intensity.value);
    document.dispatchEvent(new CustomEvent('pc-intensity',{detail:intensity.value}));});
  motion.append(intensity);body.append(motion);

  const rt=native();const apiProfile=profiles.find(p=>p.kind==='openai_compat');
  if(rt&&apiProfile){
     const api=field('Replace provider key','','password');const status=el('small','Checking native keychain');
     void rt.core.invoke<{configured:boolean;masked_tail:string|null}>('key_status',{profile:apiProfile.id}).then(s=>{
       status.textContent=s.configured?('Configured '+(s.masked_tail??'')):'Not configured';
     }).catch(problem);
     body.append(status,api,button('Save to keychain',()=>{const value=input(api).value;input(api).value='';
       void rt.core.invoke<{configured:boolean;masked_tail:string|null}>('save_key',{profile:apiProfile.id,value})
         .then(s=>{status.textContent=s.configured?('Configured '+(s.masked_tail??'')):'Not configured';}).catch(problem);
     }));
  }
  open('Settings · Usage & limits',body);
  document.dispatchEvent(new Event('pc-settings'));
  const observed=usage.reduce((n,item)=>Math.max(n,item.context??0),0);
  document.dispatchEvent(new CustomEvent('pc-context',{detail:observed/512000}));
}
byId('usage').addEventListener('click',()=>{
  void host<ProviderUsage[]>('/api/usage').then(values=>{usage=values;renderQuotas();}).catch(()=>{}).finally(settings);
});
byId('register').addEventListener('click',()=>{
  const name=field('Project name');const path=field('Local repository path');
  open('Register project',name,path,button('Register',()=>{void(async()=>{
    const s=await host<SessionDto>('/api/sessions',{name:input(name).value,repo_cwd:input(path).value});
    selected=s.id;dialog.close();pane('chat');await sync();
  })().catch(problem);},'primary'));
});
byId<HTMLSelectElement>('provider').addEventListener('change',event=>{
  const value=(event.target as HTMLSelectElement).value.split('|');
  if(selected&&value.length===2)void host(sessionPath()+'/profile',{profile_id:value[0],model:value[1]}).catch(problem);
});
byId('compose').addEventListener('submit',event=>{
  event.preventDefault();const user=byId<HTMLTextAreaElement>('prompt');const text=user.value.trim();if(!text||!selected)return;
  if(!connection){tell('Connect to the local daemon first');return;}
  void host(sessionPath()+'/messages',{request_id:crypto.randomUUID(),text}).then(()=>{
    if(user.value.trim()===text)user.value='';document.dispatchEvent(new Event('pc-start'));queueSync();
  }).catch(problem);
});
byId('auto').addEventListener('click',()=>{const b=byId('auto');const automatic=b.getAttribute('aria-pressed')!=='true';b.setAttribute('aria-pressed',String(automatic));b.textContent=automatic?'◆ Auto':'◇ Manual';});
function tokenDialog():void{
  const token=field('Local daemon token','','password');
  open('Connect to the local daemon',token,button('Connect',()=>{
    const value=input(token).value.trim();
    if(!/^[0-9a-f]{32,64}$/i.test(value)||location.hostname!=='127.0.0.1'){tell('Use the local daemon address and token');return;}
    sessionStorage.setItem('pc-session',value);input(token).value='';location.reload();
  },'primary'));
}
void (async()=>{
  renderQuotas();updateSidebar();renderChat();await renderInspector();
  try{connection=await bootstrap();}catch(e){problem(e);return;}
  if(connection){await sync();tell('Local daemon connected');void follow(connection,frame,current.signal);}
  else {tell('Local daemon not connected');tokenDialog();}
})().catch(problem);
window.addEventListener('pagehide',()=>current.abort(),{once:true});
