import { copyFile, cp } from 'node:fs/promises';
await copyFile('index.html','dist/index.html');
await cp('src/styles','dist/styles',{recursive:true});
