export interface Connection {origin:string; bearer:string}
type Tauri={core:{invoke<T>(command:string,args?:Record<string,unknown>):Promise<T>}};
export const native=()=> (window as Window & {__TAURI__?:Tauri}).__TAURI__;
/** The loopback token the desktop page gets from `#token=`; lives in this tab only. */
const SESSION_KEY='pc-session';
/** A paired device token; the phone keeps it so a reload reconnects without the QR. */
const DEVICE_KEY='pc-device';
const loopbackHost=()=>location.hostname==='127.0.0.1'||location.hostname==='[::1]';
/**
 * Endpoints this page may call: the daemon on loopback (desktop page and the
 * Tauri shell) and, on a page served over https — the phone through Tailscale
 * Serve — exactly its own origin, never another site.
 */
export function trustedOrigin(origin:string):boolean{
  if(/^http:\/\/127\.0\.0\.1:\d+$/.test(origin))return true;
  return location.protocol==='https:' && origin===location.origin;
}
async function exchangePairing(origin:string,secret:string):Promise<string|null>{
  try{
    const res=await fetch(origin+'/api/pairing/exchange',{
      method:'POST',redirect:'error',headers:{'Content-Type':'application/json'},
      body:JSON.stringify({secret}),
    });
    if(!res.ok)return null;
    const body=await res.json() as {token?:unknown};
    return typeof body.token==='string' && /^[0-9a-f]{32,64}$/i.test(body.token)?body.token:null;
  }catch{return null;}
}
export async function bootstrap():Promise<Connection|null>{
  const runtime=native();
  if(runtime){const value=await runtime.core.invoke<Connection>('local_connection');return value.origin==='http://127.0.0.1:4310'?value:null;}
  const url=new URL(location.href);
  const params=new URLSearchParams(url.hash.slice(1));
  const pair=params.get('pair');
  const supplied=params.get('token');
  // The pairing secret is one-time and must not survive in the address bar, so
  // it is exchanged and stripped before anything else happens.
  if(url.hash)history.replaceState(null,'',url.pathname+url.search);
  if(pair){
    if(location.protocol!=='https:')return null;
    const token=await exchangePairing(url.origin,pair);
    if(!token)return null;
    try{localStorage.setItem(DEVICE_KEY,token);}catch{return null;}
    return {origin:url.origin,bearer:token};
  }
  if(location.protocol==='https:'){
    let bearer:string|null=null;
    try{bearer=localStorage.getItem(DEVICE_KEY);}catch{bearer=null;}
    if(supplied && /^[0-9a-f]{32,64}$/i.test(supplied)){bearer=supplied;try{localStorage.setItem(DEVICE_KEY,supplied);}catch{/* keep in memory */}}
    return bearer?{origin:url.origin,bearer}:null;
  }
  if(url.protocol!=='http:' || !loopbackHost())return null;
  if(supplied && /^[0-9a-f]{32,64}$/i.test(supplied))sessionStorage.setItem(SESSION_KEY,supplied);
  const bearer=sessionStorage.getItem(SESSION_KEY);
  return bearer?{origin:url.origin,bearer}:null;
}
/** The daemon rotated our credential (a restart): the page must re-authenticate. */
export class Unauthorized extends Error{
  constructor(){super('The daemon did not accept this credential');this.name='Unauthorized';}
}
/** Re-reads the connection the way the page did on load, so a restarted daemon's
 * new token (or a revoked device) is picked up without a stale credential. */
export async function reconnect():Promise<Connection|null>{
  const runtime=native();
  if(runtime){try{const value=await runtime.core.invoke<Connection>('local_connection');return value.origin==='http://127.0.0.1:4310'?value:null;}catch{return null;}}
  return bootstrap();
}
export async function request<T>(c:Connection,path:string,body?:unknown):Promise<T>{
  if(!path.startsWith('/api/') || !trustedOrigin(c.origin))throw Error('Untrusted local endpoint');
  const res=await fetch(c.origin+path,{
    redirect:'error',
    headers:{Authorization:'Bearer '+c.bearer,...(body===undefined?{}:{'Content-Type':'application/json'})},
    method:body===undefined?'GET':'POST',body:body===undefined?undefined:JSON.stringify(body),
  });
  if(res.status===401)throw new Unauthorized();
  if(!res.ok){
    let detail='';
    try{const body=await res.json() as {message?:unknown};if(typeof body.message==='string')detail=': '+body.message.slice(0,200);}catch{/* no JSON body */}
    throw Error('Local daemon returned '+res.status+detail);
  }
  return await res.json() as T;
}
export interface Frame{event:string;id:string|null;data:unknown}
export async function follow(c:Connection,onFrame:(event:Frame)=>Promise<void>,signal:AbortSignal):Promise<void>{
  if(!trustedOrigin(c.origin))throw Error('Untrusted local endpoint');
  let cursor:string|null=null;
  let delay=150;
  while(!signal.aborted){
    let reader:ReadableStreamDefaultReader<Uint8Array>|undefined;
    try{
      const headers:Record<string,string>={Authorization:'Bearer '+c.bearer,Accept:'text/event-stream'};
      if(cursor!==null)headers['Last-Event-ID']=cursor;
      const res=await fetch(c.origin+'/api/events',{headers,signal,redirect:'error'});
      if(res.status===401)throw new Unauthorized();
      if(!res.ok||!res.body||!res.headers.get('content-type')?.startsWith('text/event-stream'))throw Error('Event stream unavailable');
      reader=res.body.getReader();
      const decoder=new TextDecoder();
      let buffer='';
      let event='message',id:string|null=null,lines:string[]=[];
      while(!signal.aborted){
        const chunk=await reader.read();
        if(chunk.done)break;
        buffer+=decoder.decode(chunk.value,{stream:true});
        if(buffer.length>1048576)throw Error('Event exceeds limit');
        let end:number;
        while((end=buffer.indexOf('\n'))>=0){
          const line=buffer.slice(0,end).replace(/\r$/,'');
          buffer=buffer.slice(end+1);
          if(line===''){
            if(lines.length){
              const data=JSON.parse(lines.join('\n')) as unknown;
              await onFrame({event,id,data});
              if(event==='resync_required'){cursor=null;break;}
              if(id!==null)cursor=id;
              delay=150;
            }
            event='message';id=null;lines=[];
          }else if(line.startsWith('data:'))lines.push(line.slice(5).replace(/^ /,''));
          else if(line.startsWith('event:'))event=line.slice(6).trim();
          else if(line.startsWith('id:')){const next=line.slice(3).trim();if(/^\d+$/.test(next))id=next;}
          if(lines.length>256)throw Error('Event exceeds line limit');
        }
      }
    }catch(e){
      if(signal.aborted)return;
      // A rejected credential is not a transient failure: stop retrying it and
      // let the page re-authenticate.
      if(e instanceof Unauthorized)throw e;
    }
    finally{void reader?.cancel().catch(()=>{});}
    await new Promise<void>(resolve=>setTimeout(resolve,delay));
    delay=Math.min(delay*2,2500);
  }
}
