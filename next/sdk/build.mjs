#!/usr/bin/env node
import {build} from "esbuild";
import {readFile,writeFile,mkdir,readdir} from "node:fs/promises";
import path from "node:path";
const root=path.resolve(process.argv[2]??process.cwd()),out=path.join(root,"dist"),config=JSON.parse(await readFile(path.join(root,"app.json"),"utf8"));
await mkdir(out,{recursive:true});
await build({entryPoints:[path.join(root,config.client)],outfile:path.join(out,"client.js"),bundle:true,format:"esm",platform:"browser",target:"es2023",jsx:"automatic",jsxImportSource:"@dar/app-sdk",minify:true,plugins:[{name:"dar-sdk",setup(build){build.onResolve({filter:/^@dar\/app-sdk(?:\/jsx-runtime)?$/},()=>({path:"/sdk.js",external:true}));}}]});
if(config.server)await build({entryPoints:[path.join(root,config.server)],outfile:path.join(out,"server.js"),bundle:true,format:"esm",platform:"node",target:"node24"});
const files=(await readdir(out)).filter(s=>/^(client|server)\.(js|css)$/.test(s));
await writeFile(path.join(out,"dar-app.json"),JSON.stringify({...config,client:"client.js",server:config.server?"server.js":undefined,styles:files.filter(s=>s.endsWith(".css")),files},null,2));
console.info(`Built ${config.id} at ${out}`);
