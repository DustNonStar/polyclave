// Round trip for the pairing QR: the built renderPairingMatrix draws each URI as SVG in
// a real browser, the browser rasterizes it, and jsQR (an independent decoder) must read
// back the exact bytes. Also checks the quiet zone and the error correction level.
import assert from 'node:assert/strict';
import { randomBytes, randomUUID } from 'node:crypto';
import { createServer } from 'node:http';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import jsQR from 'jsqr';
import { chromium } from 'playwright';

const root=resolve('dist');
// Same shape as bins/polyclaved/src/hub_api/http.rs: ticket = UUIDv4 simple (32 hex), id = 'p' + 8 hex.
const daemonUri=()=>'polyclave://pair?ticket='+randomUUID().replaceAll('-','')+'&id=p'+randomBytes(4).toString('hex');
const uris=[
  'polyclave://pair?ticket=x&id=p1',
  'polyclave://pair?ticket=fixture-ticket&id=p1',
  daemonUri(),daemonUri(),daemonUri(),
  'polyclave://pair?ticket=ffffffffffffffffffffffffffffffff&id=pffffffff',
  // Headroom past today's 69-byte URI, with reserved URI characters.
  'polyclave://pair?ticket='+'0123456789abcdef'.repeat(8)+'&id=p00000000&label=My%20iPhone+15#x',
];
const server=createServer(async(req,res)=>{
  const path=new URL(req.url??'/','http://127.0.0.1').pathname;
  if(path==='/qr.html'){res.setHeader('content-type','text/html');res.end('<!doctype html><meta charset="utf-8"><body></body>');return;}
  try{
    const exact=resolve(root,path.slice(1));
    if(!exact.startsWith(root+'/')&&!exact.startsWith(root+'\\')){res.writeHead(403).end();return;}
    res.setHeader('content-type',exact.endsWith('.js')?'text/javascript':'application/octet-stream');
    res.end(await readFile(exact));
  }catch{res.writeHead(404).end();}
});
await new Promise(done=>server.listen(0,'127.0.0.1',done));
const origin='http://127.0.0.1:'+server.address().port;
const {POLYCLAVE_CHROMIUM,POLYCLAVE_BROWSER_CHANNEL}=process.env;
const browser=await chromium.launch(POLYCLAVE_CHROMIUM?{executablePath:POLYCLAVE_CHROMIUM}:POLYCLAVE_BROWSER_CHANNEL==='chrome'?{channel:'chrome'}:{});
try{
  const page=await browser.newPage();
  const outside=[];
  await page.route('**/*',async route=>{
    if(!route.request().url().startsWith(origin+'/')){outside.push(route.request().url());await route.abort();return;}
    await route.continue();
  });
  await page.goto(origin+'/qr.html');
  for(const uri of uris){
    const scale=4;
    const drawn=await page.evaluate(async([uri,scale])=>{
      const {renderPairingMatrix}=await import('/assets/views/qr.js');
      const svg=renderPairingMatrix(uri);
      const [,, w, h]=svg.getAttribute('viewBox').split(' ').map(Number);
      const dark=[...svg.querySelectorAll('rect')].map(r=>[Number(r.getAttribute('x')),Number(r.getAttribute('y'))]);
      // Rasterize exactly as styled on the page: black modules on white.
      svg.setAttribute('xmlns','http://www.w3.org/2000/svg');
      svg.setAttribute('width',String(w*scale));svg.setAttribute('height',String(h*scale));
      const bg=document.createElementNS('http://www.w3.org/2000/svg','rect');
      bg.setAttribute('width','100%');bg.setAttribute('height','100%');bg.setAttribute('fill','#fff');
      svg.insertBefore(bg,svg.firstChild);
      const img=new Image();
      img.src='data:image/svg+xml;charset=utf-8,'+encodeURIComponent(new XMLSerializer().serializeToString(svg));
      await img.decode();
      const canvas=document.createElement('canvas');canvas.width=w*scale;canvas.height=h*scale;
      const ctx=canvas.getContext('2d');ctx.drawImage(img,0,0);
      const data=ctx.getImageData(0,0,canvas.width,canvas.height).data;
      return {size:w,square:w===h,className:svg.getAttribute('class'),role:svg.getAttribute('role'),label:svg.getAttribute('aria-label'),dark,width:canvas.width,height:canvas.height,pixels:Array.from(data)};
    },[uri,scale]);
    assert.equal(drawn.square,true);
    assert.deepEqual([drawn.className,drawn.role,drawn.label],['qr-svg','img','One-time pairing QR code']);
    const quiet=4,count=drawn.size-quiet*2;
    assert.equal((count-17)%4,0,'module count is 4·version+17');
    const version=(count-17)/4;
    // Quiet zone: no dark module within 4 modules of the edge.
    assert.ok(drawn.dark.every(([x,y])=>x>=quiet&&y>=quiet&&x<quiet+count&&y<quiet+count));
    // Format information (ISO/IEC 18004 §7.9) around the top-left finder: EC level M or higher.
    const isDark=(x,y)=>drawn.dark.some(([dx,dy])=>dx===x+quiet&&dy===y+quiet);
    const spots=[[8,0],[8,1],[8,2],[8,3],[8,4],[8,5],[8,7],[8,8],[7,8],[5,8],[4,8],[3,8],[2,8],[1,8],[0,8]];
    let format=0;spots.forEach(([x,y],i)=>{if(isDark(x,y))format|=1<<i;});
    const ecBits=(format^0x5412)>>13;
    const level={1:'L',0:'M',3:'Q',2:'H'}[ecBits];
    assert.ok(['M','Q','H'].includes(level),'error correction level '+level);
    const result=jsQR(Uint8ClampedArray.from(drawn.pixels),drawn.width,drawn.height);
    assert.ok(result,'jsQR decoded '+uri);
    assert.deepEqual(Buffer.from(result.binaryData),Buffer.from(uri,'latin1'));
    assert.equal(result.data,uri);
    assert.equal(result.version,version);
    console.log('qr round trip ok: '+uri.length+' bytes, version '+version+', level '+level);
  }
  assert.deepEqual(outside,[]);
}finally{
  await browser.close();
  await new Promise(done=>server.close(done));
}
