import type { FrameClock } from './scheduler.js';
type Sprite={x:number;color:string;hint:string;art:readonly string[]};
const sprites:Sprite[]=[
 {x:0,color:'#4888df',hint:'#9de6ff',art:[
  '......1111......','....11111111....','..111111111111..','111111111121111.','1111111111111111',
  '.11111111111111.','...1111111111...','......1111......']},
 {x:20,color:'#d9dae9',hint:'#9c99cb',art:[
  '..11.....11.....','..111...111.....','....11111111....','..1111111211....','111111111111....',
  '..111111111111..','...1.1..1.1.....','...1.1..1.1.....']},
 {x:40,color:'#d98970',hint:'#713f48',art:[
  '......11111.....','.....1111111....','....11112121....','...1111111111...','..111111111111..',
  '..111111111111..','...1.1..1.1.....','...1.1..1.1.....']},
 {x:60,color:'#a88be7',hint:'#644381',art:[
  '....11....11....','....111..111....','....11111111....','...1112112111...','...111111111111.',
  '..11111111111...','...1.1..1.1.....','...1.1..1.1.....']},
];
export function mountPets(canvas:HTMLCanvasElement,clock:FrameClock):void{
  canvas.width=81;canvas.height=21;
  const ctx=canvas.getContext('2d');if(!ctx)return;
  let state='idle',start=0;
  function draw(t:number):void{
    if(!ctx)return;
    ctx.clearRect(0,0,81,21);ctx.imageSmoothingEnabled=false;
    const secs=t*.001;
    const overlap=!clock.disabled&&state==='thinking'&&Math.floor(secs/3)%2===1;
    for(let i=0;i<sprites.length;i++){
      const pet=sprites[i];if(!pet)continue;
      const stacking=overlap&&i===3;
      const left=stacking?43:pet.x;
      const hop=clock.disabled?0:Math.round((state==='review'?3:2)*Math.max(0,Math.sin(secs*4+i*1.21)));
      const y=(stacking?1:12)-hop;
      for(let row=0;row<pet.art.length;row++){
        const line=pet.art[row];if(!line)continue;
        for(let col=0;col<line.length;col++){const pixel=line[col];if(pixel==='.'){continue;}
          ctx.fillStyle=pixel==='2'?pet.hint:pet.color;
          ctx.fillRect(left+col,y+row,1,1);
        }
      }
      if(i===0){
        ctx.fillStyle='#85cffa';
        if(t>0&&state!=='idle'&&!clock.disabled){
          const phase=Math.floor((t-start)/33)%16;
          const h=Math.round(phase<8?phase/2:(16-phase)/2);
          ctx.fillRect(left+9,y-2-h,1,1);ctx.fillRect(left+7-h,y-1-h,1,1);ctx.fillRect(left+11+h,y-1-h,1,1);
          ctx.fillStyle=pet.color;ctx.fillRect(left+1,y+3-(phase%2),2,1);
        }
      }
      if(i===3&&state==='review'){ctx.fillStyle='#f8a583';ctx.fillRect(left+15,y-4,1,2);ctx.fillRect(left+15,y-1,1,1);}
    }
    if(state==='idle'){ctx.fillStyle='#a7b7e5';ctx.font='6px monospace';ctx.fillText('zZ',72,8);}
  }
  const change=(s:string)=>{state=s;start=performance.now();if(clock.disabled)draw(0);};
  document.addEventListener('pc-start',()=>change('thinking'));
  document.addEventListener('pc-stream',()=>change('running'));
  document.addEventListener('pc-idle',()=>change('idle'));
  document.addEventListener('pc-review',()=>change('review'));
  draw(0);clock.subscribe(t=>draw(t),()=>draw(0));
}
