export interface Connection {origin:string; bearer:string}
type Tauri={core:{invoke<T>(command:string,args?:Record<string,unknown>):Promise<T>}};
export const native=()=> (window as Window & {__TAURI__?:Tauri}).__TAURI__;
export async function bootstrap():Promise<Connection|null>{
  const runtime=native();
  if(runtime){const value=await runtime.core.invoke<Connection>('local_connection');return value.origin==='http://127.0.0.1:4310'?value:null;}
  const url=new URL(location.href);
  if(url.protocol!=='http:' || url.hostname!=='127.0.0.1')return null;
  const supplied=new URLSearchParams(url.hash.slice(1)).get('token');
  if(url.hash)history.replaceState(null,'',url.pathname+url.search);
  if(supplied && /^[0-9a-f]{32,64}$/i.test(supplied))sessionStorage.setItem('pc-session',supplied);
  const bearer=sessionStorage.getItem('pc-session');
  return bearer?{origin:url.origin,bearer}:null;
}
export async function request<T>(c:Connection,path:string,body?:unknown):Promise<T>{
  if(!path.startsWith('/api/') || !c.origin.startsWith('http://127.0.0.1:'))throw Error('Untrusted local endpoint');
  const res=await fetch(c.origin+path,{
    redirect:'error',
    headers:{Authorization:'Bearer '+c.bearer,...(body===undefined?{}:{'Content-Type':'application/json'})},
    method:body===undefined?'GET':'POST',body:body===undefined?undefined:JSON.stringify(body),
  });
  if(!res.ok)throw Error('Local daemon returned '+res.status);
  return await res.json() as T;
}
export interface Frame{event:string;id:string|null;data:unknown}
export async function follow(c:Connection,onFrame:(event:Frame)=>Promise<void>,signal:AbortSignal):Promise<void>{
  let cursor:string|null=null;
  let delay=150;
  while(!signal.aborted){
    let reader:ReadableStreamDefaultReader<Uint8Array>|undefined;
    try{
      const headers:Record<string,string>={Authorization:'Bearer '+c.bearer,Accept:'text/event-stream'};
      if(cursor!==null)headers['Last-Event-ID']=cursor;
      const res=await fetch(c.origin+'/api/events',{headers,signal,redirect:'error'});
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
    }catch{if(signal.aborted)return;}
    finally{void reader?.cancel().catch(()=>{});}
    await new Promise<void>(resolve=>setTimeout(resolve,delay));
    delay=Math.min(delay*2,2500);
  }
}
