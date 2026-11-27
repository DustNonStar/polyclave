import { copyFile, cp, mkdir, rm, writeFile } from 'node:fs/promises';
if(process.argv.includes('--clean')){
  await rm('dist',{recursive:true,force:true});
}else{
  await copyFile('index.html','dist/index.html');
  await cp('src/styles','dist/styles',{recursive:true});
  await writeFile('dist/app.js',"import './assets/app.js';\n");
  // The page loads no external scripts: the QR encoder ships inside dist.
  await mkdir('dist/assets/vendor',{recursive:true});
  await copyFile('node_modules/qrcode-generator/dist/qrcode.mjs','dist/assets/vendor/qrcode-generator.js');
}
