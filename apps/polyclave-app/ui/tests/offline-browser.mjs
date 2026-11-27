import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { chromium, webkit } from 'playwright';

const root=resolve('dist');
const capability='a'.repeat(32);
const clients=new Set();
const session={id:'sfixture',name:'Fixture workspace',role:'hub',repo_cwd:'fixture',
 profile_id:'builtin-hub',provider_model:'default'};
const messages=[{id:1,source:'user',text:'Keep the review evidence in the Hub.',queued:false}];
let pausedState={paused:false,reason:null,updated_at:'2026-11-27T00:00:00Z'};
let stage2Active=false;
const makeCouncil=()=>({
  id:'c1',ticket_id:null,repo_cwd:'fixture',level:'direction',status:'awaiting_user',
  prompt:'Choose the Stage 2 storage layout.',escalation_reasons:['direction'],chosen_label:null,decision_note:null,
  proposals:[
    {label:'方案甲',payload:{summary:'Normalized SQLite tables',approach:'Use additive migrations',touched_paths:['crates/polyclave-store/src/stage2.rs'],public_interface_changes:false,new_dependencies:[],security_sensitive:false,beyond_request:false,open_questions:[],pros:['Clean schema'],cons:['More joins']},vendor:null,model:null},
    {label:'方案乙',payload:{summary:'Single JSON document table',approach:'Store blobs in one table',touched_paths:['crates/polyclave-store/src/writer.rs'],public_interface_changes:true,new_dependencies:[],security_sensitive:false,beyond_request:false,open_questions:[],pros:['Fast prototype'],cons:['Harder queries']},vendor:null,model:null},
  ],
  created_at:'2026-11-27T00:00:00Z',decided_at:null,
});
let councilState=makeCouncil();
const prGateState={
  id:'pg1',run_id:'r1',ticket_id:'t1',repo_slug:'owner/polyclave',branch:'agent/r1/a1',
  base_sha:'111111111111',head_sha:'222222222222',pr_number:42,pr_url:null,
  status:'approved',scope_violations:[],revise_rounds:0,auto_merge_armed:true,
  reviews:[{id:'rev1',pr_gate_id:'pg1',reviewer_vendor:'agy',reviewer_model:'gemini-4',reviewed_sha:'222222222222',verdict:'approve',comments:'Looks clean.',github_review_id:101,created_at:'2026-11-27T00:00:00Z'}],
  updated_at:'2026-11-27T00:00:00Z',
};
let observedRequests=0;
const server=createServer(async(req,res)=>{
  const url=new URL(req.url??'/','http://127.0.0.1');
  const path=url.pathname;
  if(path.startsWith('/api/')){
    if(req.headers.authorization!=='Bearer '+capability){res.writeHead(401).end();return;}
    observedRequests++;
    if(path==='/api/events'){
      res.writeHead(200,{'content-type':'text/event-stream'});
      res.write(': ready\n\n');clients.add(res);req.on('close',()=>clients.delete(res));return;
    }
    let body='';for await (const chunk of req) body+=chunk;
    res.setHeader('content-type','application/json');
    if(path==='/api/pause'&&req.method==='POST'){
      stage2Active=true;
      const parsed=body?JSON.parse(body):{};
      pausedState={paused:true,reason:parsed.reason??null,updated_at:'2026-11-27T00:01:00Z'};
      res.end(JSON.stringify(pausedState));return;
    }
    if(path==='/api/resume'&&req.method==='POST'){
      pausedState={paused:false,reason:null,updated_at:'2026-11-27T00:02:00Z'};
      res.end(JSON.stringify(pausedState));return;
    }
    if(path==='/api/councils/c1/resolve'&&req.method==='POST'){
      const parsed=JSON.parse(body||'{}');
      councilState={
        ...councilState,status:'decided',chosen_label:parsed.chosen_label??'方案甲',decision_note:parsed.decision_note??null,decided_at:'2026-11-27T00:03:00Z',
        proposals:councilState.proposals.map((p,idx)=>({...p,vendor:idx===0?'claude':'dsh',model:idx===0?'claude-sonnet':'deepseek-v4-pro'})),
      };
      res.end(JSON.stringify(councilState));return;
    }
    if(path==='/api/pairing/tickets'&&req.method==='POST'){
      res.end(JSON.stringify({ticket:{id:'p1',client_kind:'ios',device_label:'Mobile client',expires_at:'2026-11-27T00:10:00Z',used_at:null,created_at:'2026-11-27T00:00:00Z'},secret:'fixture-ticket',qr_uri:'polyclave://pair?ticket=fixture-ticket&id=p1'}));return;
    }
    if(path==='/api/notifications/test'&&req.method==='POST'){
      res.end(JSON.stringify({delivered:1}));return;
    }
    const data=path==='/api/sessions'?[session]:path==='/api/profiles'?[{id:'builtin-hub',kind:'codex_cli',display_name:'Codex CLI',models:['default']},{id:'deepseek-api',kind:'openai_compat',display_name:'DeepSeek API',models:['deepseek-flash','deepseek-v4-pro']}]:
      path.endsWith('/messages')?messages:path==='/api/usage'?[{vendor:'DeepSeek',context:240000,tokens:32000,cost:.37}]:
      path==='/api/councils'?(stage2Active?[councilState]:[]):path==='/api/pr-gates'?(stage2Active?[prGateState]:[]):path==='/api/pause'?pausedState:[];
    res.end(JSON.stringify(data));return;
  }
  try{
    const file=path==='/'?'index.html':path.slice(1);
    const exact=resolve(root,file);
    if(!exact.startsWith(root+'/')&&!exact.startsWith(root+'\\')){res.writeHead(403).end();return;}
    res.setHeader('content-type',file.endsWith('.css')?'text/css':file.endsWith('.js')?'text/javascript':'text/html');
    res.end(await readFile(exact));
  }catch{res.writeHead(404).end();}
});
await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));
const origin='http://127.0.0.1:'+server.address().port;
const makeEvent=(kind,text)=>'event: agent\ndata: '+JSON.stringify({run_id:'sfixture',message:{kind,text}})+'\n\n';
function send(kind,text){for(const c of clients)c.write(makeEvent(kind,text));}
const engines=[['chromium',chromium],...(process.env.POLYCLAVE_WEBKIT==='1'?[['webkit',webkit]]:[])];
try{
 for(const [engine,type] of engines){
  const {POLYCLAVE_CHROMIUM, POLYCLAVE_BROWSER_CHANNEL}=process.env;
  const options=engine==='chromium'?
    (POLYCLAVE_CHROMIUM?{executablePath:POLYCLAVE_CHROMIUM}:POLYCLAVE_BROWSER_CHANNEL==='chrome'?{channel:'chrome'}:{}):{};
  const browser=await type.launch(options);
  try{
   for(const dims of [[1440,900],[375,812]]){
    stage2Active=false;councilState=makeCouncil();
    const [width,height]=dims;
    const page=await browser.newPage({viewport:{width,height}});
    const errors=[];const outside=[];const responses=[];
    page.on('response',r=>responses.push([new URL(r.url()).pathname,r.status()]));
    page.on('pageerror',error=>errors.push(error.message));
    await page.route('**/*', async route=>{
      if(!route.request().url().startsWith(origin+'/')){
        outside.push(route.request().url());await route.abort();return;
      }
      await route.continue();
    });
    await page.goto(origin+'/#token='+capability);
    try{await page.waitForFunction(()=>document.getElementById('connection')?.textContent==='Local daemon connected',null,{timeout:5000});}
    catch(e){throw Error(JSON.stringify({status:await page.locator('#connection').textContent(),errors,outside,responses}),{cause:e});}
    assert.equal(new URL(page.url()).hash,'');
    // Custom provider listbox: DeepSeek API first, deepseek-flash (default) before deepseek-v4-pro.
    const options=await page.locator('#provider [role=option]').evaluateAll(list=>list.map(opt=>[opt.dataset.value,opt.textContent]));
    assert.deepEqual(options.filter(([value])=>value.startsWith('deepseek-api|')),[['deepseek-api|deepseek-flash','DeepSeek API · deepseek-flash (default)'],['deepseek-api|deepseek-v4-pro','DeepSeek API · deepseek-v4-pro']]);
    assert.deepEqual(options[0],['deepseek-api|deepseek-flash','DeepSeek API · deepseek-flash (default)']);
    assert.equal(await page.locator('#provider select, #compose textarea[style*="resize: both"]').count(),0);
    // The register control carries exactly one plus icon and no duplicated glyph.
    assert.deepEqual(await page.locator('#register').evaluate(b=>[b.querySelectorAll('svg').length,b.textContent.trim()]),[1,'']);
    assert.equal(await page.locator('#prompt').evaluate(t=>getComputedStyle(t).resize),'none');
    assert.deepEqual(await page.locator('#inspect-tabs [role=tab]').allTextContents(),['Topology','Review','Files','Memory']);
    assert.deepEqual(await page.locator('#quota .q').evaluateAll(q=>q.map(e=>e.dataset.vendor)),['claude','codex','gemini','deepseek']);
    assert.ok((await page.locator('#quota [data-vendor=claude]').getAttribute('title')).includes('not reported'));

    await page.waitForFunction(()=>document.documentElement.dataset.motion==='0fps');
    assert.equal(await page.locator('#aurora').evaluate(c=>c.width),0);
    const measured=await page.evaluate(()=>{
      const canvas=document.querySelector('#aurora');
      const rect=canvas.getBoundingClientRect();
      const violations=[...document.querySelectorAll('body *')].filter(e=>{
        const s=getComputedStyle(e);return e.clientWidth>0 && e.scrollWidth>e.clientWidth+1 &&
          s.overflowX==='visible' && e.getBoundingClientRect().right>innerWidth+1;
      }).map(e=>e.id||e.tagName);
      return {width:rect.width,left:rect.left,viewport:innerWidth,body:document.documentElement.scrollWidth,violations};
    });
    assert.deepEqual(measured.violations,[]);
    assert.equal(measured.width,width);assert.equal(measured.left,0);assert.equal(measured.body,width);
    assert.ok(await page.locator('#pet-stage').evaluate(c=>c.getContext('2d').getImageData(0,0,c.width,c.height).data.some(v=>v!==0)));
    // The aurora never takes pointer events and sits beneath the workspace.
    assert.deepEqual(await page.evaluate(()=>{const a=getComputedStyle(document.querySelector('#aurora'));return [a.pointerEvents,a.position,Number(a.zIndex)<Number(getComputedStyle(document.querySelector('#app')).zIndex)];}),['none','fixed',true]);
    if(width===375){
      const small=await page.evaluate(()=>[...document.querySelectorAll('button,[role=option],[role=tab]')].filter(b=>{
        const r=b.getBoundingClientRect();return r.width>0&&r.height>0&&getComputedStyle(b).visibility!=='hidden'&&r.bottom>0&&r.top<innerHeight&&r.height<44;
      }).map(b=>b.id||b.textContent.trim().slice(0,24)));
      assert.deepEqual(small,[]);
    }
    if(clients.size===0)await new Promise((resolve,reject)=>{
      const stop=setTimeout(()=>{clearInterval(check);reject(Error('No local SSE client'));},3000);
      const check=setInterval(()=>{if(clients.size){clearTimeout(stop);clearInterval(check);resolve();}},15);
    });
    const supported=await page.evaluate(()=>typeof CSS.highlights!=='undefined');
    if(supported){
      await page.evaluate(()=>{
        window.highlightObserved=false;
        const watch=new MutationObserver(()=>{
          const check=()=>{if(CSS.highlights.size>0)window.highlightObserved=true;else if(document.querySelector('#stream')?.textContent)requestAnimationFrame(check);};
          requestAnimationFrame(check);
        });
        watch.observe(document.querySelector('#stream'),{childList:true});
      });
    }
    send('text_delta','Offline ink sample');
    await page.locator('#stream').getByText('Offline ink sample',{exact:true}).waitFor({state:'visible'});
    await page.waitForFunction(()=>document.documentElement.dataset.motion!=='0fps');
    assert.ok(await page.locator('#aurora').evaluate(c=>c.width>0));
    if(supported)await page.waitForFunction(()=>window.highlightObserved===true);
    send('finished');
    await page.waitForFunction(()=>document.documentElement.dataset.motion==='0fps');
    if(supported)assert.equal(await page.evaluate(()=>CSS.highlights.size),0);
    if(width===375)await page.getByRole('button',{name:'Workspace',exact:true}).click();
    await page.getByRole('button',{name:'Light theme',exact:true}).click();
    assert.equal(await page.locator('html').getAttribute('data-theme'),'light');
    assert.equal(await page.locator('html').evaluate(()=>document.documentElement.scrollWidth),width);
    await page.getByRole('button',{name:'Settings · Usage & limits',exact:true}).click();
    await page.getByRole('dialog').waitFor({state:'visible'});
    assert.deepEqual(await page.getByRole('radiogroup',{name:'Motion intensity'}).getByRole('radio').allTextContents(),['Off · 0fps','Subtle','Standard','Ultra']);
    assert.ok((await page.getByRole('dialog').textContent()).includes('not reported'));
    // Stage 3: generate inline SVG pairing QR & test private notification wakeup inside Settings.
    await page.locator('[data-focus-key="create-qr"]').click();
    await page.locator('.qr-svg').waitFor({state:'visible'});
    await page.locator('[data-focus-key="test-notify"]').click();
    assert.equal(await page.getByRole('dialog').evaluate(d=>d.scrollWidth<=d.clientWidth+1),true);
    await page.getByRole('button',{name:'Close',exact:true}).click();
    await page.getByRole('button',{name:'Dark theme',exact:true}).click();
    assert.equal(await page.locator('html').getAttribute('data-theme'),'dark');
    await page.emulateMedia({reducedMotion:'reduce'});
    if(width===375)await page.getByRole('button',{name:'Hub',exact:true}).click();
    send('text_delta','Reduced-motion text');
    await page.locator('#stream').getByText('Reduced-motion text',{exact:true}).waitFor({state:'visible'});
    assert.equal(await page.locator('html').getAttribute('data-motion'),'0fps');
    // Stage 2: Global Pause toggle, Blind Council & PR Gates views.
    await page.locator('#pause-toggle').click();
    await page.waitForFunction(()=>document.getElementById('pause-toggle')?.getAttribute('aria-pressed')==='true');
    assert.ok((await page.locator('#counts').textContent()).includes('Paused'));
    await page.locator('#pause-toggle').click();
    await page.waitForFunction(()=>document.getElementById('pause-toggle')?.getAttribute('aria-pressed')==='false');
    if(width===375)await page.getByRole('button',{name:'Inspect',exact:true}).click();
    await page.locator('#tab-review').click();
    await page.locator('[data-review-view="council"]').click();
    await page.locator('.proposal-card').first().waitFor({state:'visible'});
    assert.equal(await page.locator('.proposal-card[data-blind="true"]').count(),2);
    if(councilState.status==='awaiting_user'){
      await page.locator('[data-focus-key="confirm-council"]').click();
      await page.waitForFunction(()=>document.querySelectorAll('.proposal-card[data-blind="false"]').length===2);
    }
    await page.locator('[data-review-view="pr_gates"]').click();
    await page.locator('.pr-gate-card').waitFor({state:'visible'});
    assert.ok((await page.locator('.pr-gate-card').textContent()).includes('auto-merge armed'));
    assert.equal(await page.locator('body').evaluate(e=>e.scrollWidth),width);
    assert.deepEqual(errors,[]);assert.deepEqual(outside,[]);
    console.log(engine+' '+width+'x'+height+' '+JSON.stringify(measured)+' highlights='+supported+' no external requests');
    await page.close();
   }
  }finally{await browser.close();}
 }
 assert.ok(observedRequests>0);
}finally{
 for(const c of clients)c.end();
 await new Promise(resolve=>server.close(resolve));
}
