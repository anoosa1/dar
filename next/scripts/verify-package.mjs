import {spawn,spawnSync} from "node:child_process";
import {mkdtemp} from "node:fs/promises";
import {tmpdir} from "node:os";
import path from "node:path";
import net from "node:net";
const executable=process.argv[2];if(!executable)throw Error("Usage: node next/scripts/verify-package.mjs /nix/store/...-dar/bin/dar");
const root=await mkdtemp(path.join(tmpdir(),"dar-package-"));
async function freePort(){const server=net.createServer();await new Promise(r=>server.listen(0,"127.0.0.1",r));const port=server.address().port;await new Promise(r=>server.close(r));return port;}
const port=await freePort(),dbPort=await freePort(),project=path.resolve(import.meta.dirname,".."),env={...process.env,DAR_ROOT:root,DAR_TEST_ROOT:root,DAR_PORT:String(port),DAR_DATABASE_PORT:String(dbPort),DAR_DATABASE_NAME:"dar-package",DAR_INTEGRATION_ORIGIN:`http://127.0.0.1:${port}`,DAR_DATABASE_URL:`http://127.0.0.1:${dbPort}`,DAR_DATABASE_TOKEN_FILE:path.join(root,"secrets/database-token"),DAR_BOOTSTRAP_TOKEN_FILE:path.join(root,"secrets/bootstrap-token"),DAR_SPACETIME_CLI_ROOT:path.join(root,"cli")};
const child=spawn(executable,["--development"],{env,stdio:["ignore","pipe","pipe"]});let output="";for(const stream of[child.stdout,child.stderr])stream.on("data",chunk=>output=(output+chunk).slice(-6000));
try{let ready=false;for(let i=0;i<300;i++){if(child.exitCode!==null)throw Error(output);try{ready=(await fetch(env.DAR_INTEGRATION_ORIGIN+"/api/v1/auth/status",{signal:AbortSignal.timeout(500)})).ok;}catch{}if(ready)break;await new Promise(r=>setTimeout(r,100));}if(!ready)throw Error(output);
  for(const args of[["--test","test/auth.integration.test.ts"],["--test","test/davx5.integration.test.ts"],["test/pim-dav-client.mjs"]]){const result=spawnSync(process.execPath,args,{cwd:project,env,stdio:"inherit",timeout:120000});if(result.error)throw result.error;if(result.status!==0)throw Error("Packaged acceptance failed: "+args.join(" "));}
  console.info("PASS immutable Nix package starts without builds/install, initializes private state and serves DAV client round trips. Root: "+root);
}finally{if(child.exitCode===null){child.kill("SIGTERM");await new Promise(resolve=>{const timer=setTimeout(()=>child.kill("SIGKILL"),15000);child.once("exit",()=>{clearTimeout(timer);resolve();});});}}
