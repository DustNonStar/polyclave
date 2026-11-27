import type { FrameClock } from './scheduler.js';
const vertex=`#version 300 es
in vec2 point; out vec2 uv;
void main(){uv=point*0.5+0.5;gl_Position=vec4(point,0.,1.);}`;
const fragment=`#version 300 es
precision mediump float;
in vec2 uv;out vec4 shade;uniform float time;uniform float lightState;
float ribbon(float center,float width){float d=(uv.y-center)/width;return exp(-d*d);}
void main(){
  float xx=uv.x;
  float bend=.12+.048*sin(9.*xx+time*.61)+.023*sin(18.*xx-time*.52);
  float bend2=.17+.07*sin(7.*xx-time*.74)+.025*cos(20.*xx+time*.38);
  float gate=smoothstep(0.,.18,xx)*smoothstep(0.,.23,1.-xx);
  float end=smoothstep(0.,.17,uv.y)*(1.-smoothstep(.27,.58,uv.y));
  vec3 blue=mix(vec3(.23,.36,.90),vec3(.39,.52,.87),lightState);
  vec3 cyan=vec3(.28,.74,.88),violet=vec3(.60,.43,.90);
  vec3 outColor=blue*ribbon(bend,.066)+cyan*ribbon(bend2,.043)*.55+
      violet*ribbon(bend+.13+.04*sin(xx*12.),.085)*.49;
  float a=min(length(outColor)*gate*end*.44,.49);
  shade=vec4(min(outColor,vec3(1.)),a);
}`;
function compile(gl:WebGL2RenderingContext,type:number,source:string):WebGLShader|null{
  const s=gl.createShader(type);if(!s)return null;
  gl.shaderSource(s,source);gl.compileShader(s);return gl.getShaderParameter(s,gl.COMPILE_STATUS)?s:null;
}
export function mountAurora(canvas:HTMLCanvasElement,clock:FrameClock):void{
  const gl=canvas.getContext('webgl2',{alpha:true,antialias:false,premultipliedAlpha:false});
  let useGl=false;let tLoc:WebGLUniformLocation|null=null,lLoc:WebGLUniformLocation|null=null;
  if(gl){
    const vs=compile(gl,gl.VERTEX_SHADER,vertex),fs=compile(gl,gl.FRAGMENT_SHADER,fragment),program=gl.createProgram();
    if(vs&&fs&&program){
      gl.attachShader(program,vs);gl.attachShader(program,fs);gl.linkProgram(program);
      if(gl.getProgramParameter(program,gl.LINK_STATUS)){
        gl.useProgram(program);const b=gl.createBuffer();gl.bindBuffer(gl.ARRAY_BUFFER,b);
        gl.bufferData(gl.ARRAY_BUFFER,new Float32Array([-1,-1,3,-1,-1,3]),gl.STATIC_DRAW);
        const p=gl.getAttribLocation(program,'point');
        gl.enableVertexAttribArray(p);gl.vertexAttribPointer(p,2,gl.FLOAT,false,0,0);
        tLoc=gl.getUniformLocation(program,'time');lLoc=gl.getUniformLocation(program,'lightState');useGl=true;
      }
    }
  }
  const fallback=gl?null:canvas.getContext('2d');
  const size=()=>[Math.min(1920,Math.floor(innerWidth*.8)),210];
  const glow=document.getElementById('afterglow');
  clock.subscribe((time)=>{
    const [width,height]=size();
    if(width===undefined||height===undefined)return;
    if(canvas.width!==width||canvas.height!==height){canvas.width=width;canvas.height=height;}
    if(gl&&useGl){
      gl.viewport(0,0,width,height);gl.uniform1f(tLoc,time*.001);
      gl.uniform1f(lLoc,document.documentElement.dataset.theme==='light'?1:0);gl.drawArrays(gl.TRIANGLES,0,3);
    }else if(fallback){
      fallback.clearRect(0,0,width,height);
      for(let i=0;i<4;i++){
        const x=width*(.15+i*.22)+Math.sin(time*.00041+i)*width*.056;
        fallback.save();fallback.translate(x,height+4);fallback.scale(width*.28,95);
        const color=fallback.createRadialGradient(0,0,0,0,0,1);
        color.addColorStop(0,['#4a63df80','#58bee966','#6279f777','#8f6ce670'][i]??'#4a63df70');
        color.addColorStop(1,'transparent');
        fallback.fillStyle=color;fallback.fillRect(-1,-1,2,2);fallback.restore();
      }
    }
    if(gl&&!useGl&&glow)glow.style.opacity=String(.66+.1*Math.sin(time*.0009));
  },()=>{canvas.width=0;canvas.height=0;if(glow)glow.style.opacity='';});
  window.addEventListener('resize',()=>clock.wake());
}
