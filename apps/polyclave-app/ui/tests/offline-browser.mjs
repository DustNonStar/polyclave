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
    res.setHeader('content-type','application/json');
    const data=path==='/api/sessions'?[session]:path==='/api/profiles'?[{id:'builtin-hub',kind:'codex_cli',display_name:'Codex CLI',models:['default']},{id:'deepseek-api',kind:'openai_compat',display_name:'DeepSeek API',models:['deepseek-flash','deepseek-v4-pro']}]:
      path.endsWith('/messages')?messages:path==='/api/usage'?[{vendor:'DeepSeek',context:240000,tokens:32000,cost:.37}]:[];
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
    const deepseek=await page.locator('#provider option').evaluateAll(options=>options.filter(opt=>opt.value.startsWith('deepseek-api|')).map(opt=>[opt.value,opt.textContent]));
    assert.deepEqual(deepseek,[['deepseek-api|deepseek-flash','DeepSeek API · deepseek-flash (default)'],['deepseek-api|deepseek-v4-pro','DeepSeek API · deepseek-v4-pro']]);

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
    await page.getByRole('button',{name:'Settings · Usage & limits',exact:true}).click();
    await page.getByRole('dialog').waitFor({state:'visible'});
    await page.getByRole('button',{name:'Close',exact:true}).click();
    await page.emulateMedia({reducedMotion:'reduce'});
    if(width===375)await page.getByRole('button',{name:'Hub',exact:true}).click();
    send('text_delta','Reduced-motion text');
    await page.locator('#stream').getByText('Reduced-motion text',{exact:true}).waitFor({state:'visible'});
    assert.equal(await page.locator('html').getAttribute('data-motion'),'0fps');
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
