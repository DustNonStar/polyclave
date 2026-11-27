import {play} from './spring.js';
const paths={
  topology:'M4 4h4v4H4z M16 16h4v4h-4z M8 6h10v10',
  review:'M5 3h11l3 3v15H5z M8 12l2 2 5-5',
  files:'M3 6h7l2 2h9v11H3z',
  memory:'M5 4h14v16H5z M8 8h8 M8 12h8 M8 16h6',
  send:'M12 20V4 M5 11l7-7 7 7',
  pause:'M8 5v14 M16 5v14',
  stop:'M6 6h12v12H6z',
  search:'M15 15l5 5 M16 10a6 6 0 1 1-12 0a6 6 0 0 1 12 0z',
  settings:'M4 7h16 M4 12h16 M4 17h16 M7 5v4 M16 10v4 M10 15v4',
  globe:'M22 12a10 10 0 1 1-20 0a10 10 0 0 1 20 0z M3 12h18 M12 2c-5 5-5 15 0 20 5-5 5-15 0-20',
  retry:'M3 11a9 9 0 1 1 3 8 M3 4v7h7',
  fail:'M6 6l12 12 M18 6L6 18',
  accept:'M4 12l5 5L20 6',
  copy:'M8 8h12v12H8z M4 16V4h12',
  hub:'M3 12l9-9 9 9-9 9z',
  code:'M8 5l-5 7 5 7 M16 5l5 7-5 7',
  branch:'M6 3v18 M6 12c7 0 11-5 11-9',
  light:'M12 2v3 M12 19v3 M2 12h3 M19 12h3 M17 12a5 5 0 1 1-10 0a5 5 0 0 1 10 0',
  moon:'M20 16A9 9 0 0 1 8 4a9 9 0 1 0 12 12z',
  plus:'M12 4v16 M4 12h16',
  arrow:'M4 12h16 M13 5l7 7-7 7',
} as const;
export type Icon=keyof typeof paths;
export function icon(name:Icon):SVGSVGElement{
  const ns='http://www.w3.org/2000/svg';
  const svg=document.createElementNS(ns,'svg');svg.setAttribute('viewBox','0 0 24 24');svg.setAttribute('aria-hidden','true');
  svg.classList.add('micro-icon');
  const path=document.createElementNS(ns,'path');path.setAttribute('d',paths[name]);svg.append(path);return svg;
}
export function addIcons():void{
  const map=[['theme','moon'],['usage','settings'],['register','plus']] as const;
  for(const [id,name] of map){
    const button=document.getElementById(id);if(!button)continue;
    button.prepend(icon(name));
    button.addEventListener('pointerenter',()=>play(button,[{transform:'scale(1)'},{transform:'scale(1.032)'}],'snappy',270));
  }
}
