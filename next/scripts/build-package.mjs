import {build} from "esbuild";
import {spawnSync} from "node:child_process";
import {mkdir} from "node:fs/promises";
const run=args=>{const result=spawnSync(args[0],args.slice(1),{stdio:"inherit"});if(result.error)throw result.error;if(result.status)process.exit(result.status);};
await mkdir("module/dist",{recursive:true});
// The database provides its syscall modules; all ordinary SDK code is bundled.
await build({entryPoints:["module/src/index.ts"],bundle:true,outfile:"module/dist/bundle.js",format:"esm",platform:"neutral",target:"es2022",external:["spacetime:*"]});
run([process.env.DAR_SPACETIME_BIN??"spacetime","--root-dir",process.env.DAR_SPACETIME_CLI_ROOT,"generate","--lang","typescript","--js-path","module/dist/bundle.js","--out-dir","client/module_bindings","--import-extension","ts","--yes"]);
run([process.execPath,"node_modules/typescript/bin/tsc","-p","tsconfig.json"]);
run([process.execPath,"node_modules/typescript/bin/tsc","-p","module/tsconfig.json"]);
await import("./build-client.mjs");
