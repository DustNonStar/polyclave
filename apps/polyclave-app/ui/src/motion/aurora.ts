import type { FrameClock } from './scheduler.js';

// Global horizon aurora: a soft, low-saturation glow along the bottom of the viewport.
// Output is premultiplied so WebKit composites it as light, not as a pale wash.
const vertex = `#version 300 es
in vec2 point; out vec2 uv;
void main(){ uv = point * 0.5 + 0.5; gl_Position = vec4(point, 0.0, 1.0); }`;

const fragment = `#version 300 es
precision mediump float;
in vec2 uv; out vec4 shade;
uniform float time; uniform float light; uniform float aspect;
float band(float y, float c, float w){ float d = (y - c) / w; return exp(-d * d); }
void main(){
  float x = uv.x * aspect;
  float y = uv.y;
  // Horizon floor glow.
  float floorGlow = exp(-y * 5.2) * 0.55;
  // Three slow curtains that sway above the horizon.
  float c1 = 0.16 + 0.035 * sin(x * 1.7 + time * 0.23) + 0.018 * sin(x * 4.1 - time * 0.31);
  float c2 = 0.27 + 0.045 * sin(x * 1.3 - time * 0.19 + 1.7) + 0.02 * sin(x * 3.3 + time * 0.27);
  float c3 = 0.38 + 0.05 * sin(x * 0.9 + time * 0.15 + 3.1);
  float s1 = 0.55 + 0.45 * sin(x * 2.6 + time * 0.42);
  float s2 = 0.55 + 0.45 * sin(x * 2.1 - time * 0.36 + 2.0);
  float s3 = 0.5 + 0.5 * sin(x * 1.6 + time * 0.28 + 4.0);
  vec3 cobalt = vec3(0.24, 0.40, 0.92);
  vec3 cyan   = vec3(0.30, 0.66, 0.86);
  vec3 iris   = vec3(0.50, 0.42, 0.86);
  vec3 col = cobalt * floorGlow
           + cobalt * band(y, c1, 0.075) * s1 * 0.7
           + cyan   * band(y, c2, 0.06)  * s2 * 0.45
           + iris   * band(y, c3, 0.09)  * s3 * 0.32;
  float edge = smoothstep(0.0, 0.12, uv.x) * smoothstep(0.0, 0.12, 1.0 - uv.x);
  float top = 1.0 - smoothstep(0.42, 0.95, y);
  float a = clamp(max(max(col.r, col.g), col.b), 0.0, 1.0) * edge * top * mix(0.64, 0.4, light);
  vec3 tint = normalize(col + 1e-4) * mix(1.0, 0.9, light);
  shade = vec4(tint * a, a);
}`;

function compile(gl: WebGL2RenderingContext, type: number, source: string): WebGLShader | null {
  const s = gl.createShader(type);
  if (!s) return null;
  gl.shaderSource(s, source); gl.compileShader(s);
  return gl.getShaderParameter(s, gl.COMPILE_STATUS) ? s : null;
}

export function mountAurora(canvas: HTMLCanvasElement, clock: FrameClock): void {
  const gl = canvas.getContext('webgl2', { alpha: true, antialias: false, premultipliedAlpha: true });
  let ready = false;
  let tLoc: WebGLUniformLocation | null = null, lLoc: WebGLUniformLocation | null = null, aLoc: WebGLUniformLocation | null = null;
  if (gl) {
    const vs = compile(gl, gl.VERTEX_SHADER, vertex), fs = compile(gl, gl.FRAGMENT_SHADER, fragment), program = gl.createProgram();
    if (vs && fs && program) {
      gl.attachShader(program, vs); gl.attachShader(program, fs); gl.linkProgram(program);
      if (gl.getProgramParameter(program, gl.LINK_STATUS)) {
        gl.useProgram(program);
        const buffer = gl.createBuffer(); gl.bindBuffer(gl.ARRAY_BUFFER, buffer);
        gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([-1, -1, 3, -1, -1, 3]), gl.STATIC_DRAW);
        const p = gl.getAttribLocation(program, 'point');
        gl.enableVertexAttribArray(p); gl.vertexAttribPointer(p, 2, gl.FLOAT, false, 0, 0);
        tLoc = gl.getUniformLocation(program, 'time'); lLoc = gl.getUniformLocation(program, 'light'); aLoc = gl.getUniformLocation(program, 'aspect');
        ready = true;
      }
    }
  }
  const flat = gl ? null : canvas.getContext('2d');
  // Render at reduced resolution; the glow is soft by nature and CSS stretches it to the full width.
  const size = (): [number, number] => {
    const rect = canvas.getBoundingClientRect();
    return [Math.max(1, Math.round(rect.width * 0.5)), Math.max(1, Math.round(rect.height * 0.5))];
  };
  clock.subscribe(time => {
    const [width, height] = size();
    if (canvas.width !== width || canvas.height !== height) { canvas.width = width; canvas.height = height; }
    const light = document.documentElement.dataset.theme === 'light' ? 1 : 0;
    if (gl && ready) {
      gl.viewport(0, 0, width, height);
      gl.clearColor(0, 0, 0, 0); gl.clear(gl.COLOR_BUFFER_BIT);
      gl.uniform1f(tLoc, time * 0.001); gl.uniform1f(lLoc, light); gl.uniform1f(aLoc, width / height / 4);
      gl.drawArrays(gl.TRIANGLES, 0, 3);
    } else if (flat) {
      flat.clearRect(0, 0, width, height);
      const tones = light ? ['rgba(96,140,255,.16)', 'rgba(120,190,240,.13)', 'rgba(160,140,250,.1)'] : ['rgba(61,108,240,.3)', 'rgba(76,168,220,.18)', 'rgba(128,107,220,.14)'];
      for (let i = 0; i < 3; i++) {
        const x = width * (0.2 + i * 0.3) + Math.sin(time * 0.0003 + i * 2) * width * 0.06;
        const g = flat.createRadialGradient(x, height, 0, x, height, width * 0.38);
        g.addColorStop(0, tones[i] ?? 'transparent'); g.addColorStop(1, 'rgba(0,0,0,0)');
        flat.fillStyle = g; flat.fillRect(0, 0, width, height);
      }
    }
  }, () => { canvas.width = 0; canvas.height = 0; });
  window.addEventListener('resize', () => clock.wake());
}
