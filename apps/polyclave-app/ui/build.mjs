import { copyFile, cp, rm, writeFile } from 'node:fs/promises';
if(process.argv.includes('--clean')){
  await rm('dist',{recursive:true,force:true});
}else{
  await copyFile('index.html','dist/index.html');
  await cp('src/styles','dist/styles',{recursive:true});
  await writeFile('dist/app.js',"import './assets/app.js';\n");
}
