// Optional real CLI/systemd boundary check. No model call or saved login needed.
// Run: nix develop -c node next/test/codex-sandbox-systemd.mjs /absolute/path/to/codex
import assert from "node:assert/strict";
import {mkdtemp,mkdir,writeFile,rm} from "node:fs/promises";
import {execFile} from "node:child_process";
import {promisify} from "node:util";
import path from "node:path";
import {verifyCodexSandbox} from "../runtime/agent/codex.ts";

if(process.argv[2]==="--probe"){
  const [, , , root,executable]=process.argv;
  try{await verifyCodexSandbox({executable,home:root+"/home",userId:"fixture",gitHosts:[]},root+"/work",root+"/private-sentinel");console.log("ISOLATION_OK");}
  catch(error){console.error(error.code??"unexpectedFailure");process.exitCode=1;}
}else{
  const executable=process.argv[2];assert.ok(executable&&path.isAbsolute(executable),"Supply an installed Codex CLI path.");
  const root=await mkdtemp("/tmp/dar-codex-systemd-");
  try{
    for(const directory of ["home","work","work/.git","work/.codex"])await mkdir(path.join(root,directory),{recursive:true});
    await writeFile(root+"/private-sentinel","Synthetic private data\n",{mode:0o600});
    const run=async families=>{try{return {ok:true,...await promisify(execFile)("systemd-run",["--user","--pipe","--wait","--collect","-p",`RestrictAddressFamilies=${families}`,"-p","NoNewPrivileges=yes",process.execPath,import.meta.filename,"--probe",root,executable],{encoding:"utf8",timeout:20000})};}catch(error){return {ok:false,stdout:error.stdout??"",stderr:error.stderr??""};}};
    const blocked=await run("AF_UNIX AF_INET AF_INET6");assert.equal(blocked.ok,false);assert.match(blocked.stderr,/sandboxAddressFamilyBlocked/);
    const allowed=await run("AF_UNIX AF_INET AF_INET6 AF_NETLINK");assert.equal(allowed.ok,true,allowed.stderr);assert.match(allowed.stdout,/ISOLATION_OK/);
    console.log("PASS real Codex sandbox: blocked without AF_NETLINK, isolated successfully with AF_NETLINK.");
  }finally{await rm(root,{recursive:true,force:true});}
}
