// Optional real CLI/systemd check. No model call or saved login needed.
// Run: nix develop -c node next/test/codex-sandbox-systemd.mjs /absolute/path/to/codex
import assert from "node:assert/strict";
import {readFile} from "node:fs/promises";
import {execFile} from "node:child_process";
import {promisify} from "node:util";
import path from "node:path";

const executable=process.argv[2];assert.ok(executable&&path.isAbsolute(executable),"Supply an installed Codex CLI path.");
const sources=await Promise.all(["../runtime/agent/codex.ts","../runtime/errors.ts"].map(file=>readFile(new URL(file,import.meta.url),"utf8")));
// Stage only adapter code and synthetic data inside the unit's private /tmp.
// The full unit hides /home, including this checkout, just like deployed Dar.
const bootstrap=`
  import {mkdtemp,mkdir,writeFile,rm,readFile} from "node:fs/promises";
  import {pathToFileURL} from "node:url";
  const [adapter,errors,executable,metadata]=process.argv.slice(1),root=await mkdtemp("/tmp/dar-codex-unit-");
  try{
    for(const dir of ["runtime/agent","home","work/.git",...(metadata==="missing"?[]:["work/.codex"])])await mkdir(root+"/"+dir,{recursive:true});
    await writeFile(root+"/runtime/agent/codex.ts",adapter);await writeFile(root+"/runtime/errors.ts",errors);
    await writeFile(root+"/private-sentinel","Synthetic private data\\n",{mode:0o600});
    const {verifyCodexSandbox}=await import(pathToFileURL(root+"/runtime/agent/codex.ts"));
    await verifyCodexSandbox({executable,home:root+"/home",userId:"fixture",gitHosts:[]},root+"/work",root+"/private-sentinel");
    console.log("ISOLATION_OK");
  }catch(error){console.error(error.code??"unexpectedFailure");process.exitCode=1;}
  finally{await rm(root,{recursive:true,force:true});}
`;
const properties=[
  "NoNewPrivileges=yes","PrivateTmp=yes","PrivateDevices=yes","ProtectSystem=strict",
  "ProtectHome=yes","ProtectKernelModules=yes","ProtectControlGroups=yes",
  "RestrictSUIDSGID=yes","UMask=0077","WorkingDirectory=/","TasksMax=256","MemoryMax=2G",
  // Keep a caller-supplied temporary CLI and its resources visible in PrivateTmp.
  `BindReadOnlyPaths=${path.dirname(path.dirname(executable))}`,
];
const run=async(netlink,tunables,metadata="complete")=>{
  const flags=[...properties,"RestrictAddressFamilies=AF_UNIX AF_INET AF_INET6"+(netlink?" AF_NETLINK":""),`ProtectKernelTunables=${tunables?"yes":"no"}`];
  try{return {ok:true,...await promisify(execFile)("systemd-run",["--user","--pipe","--wait","--collect","--expand-environment=no",...flags.flatMap(flag=>["-p",flag]),process.execPath,"--input-type=module","-e",bootstrap,...sources,executable,metadata],{encoding:"utf8",timeout:20000})};}
  catch(error){return {ok:false,stdout:error.stdout??"",stderr:error.stderr??""};}
};
const addressBlocked=await run(false,false);assert.equal(addressBlocked.ok,false);assert.match(addressBlocked.stderr,/sandboxAddressFamilyBlocked/);
const protectedProc=await run(true,true);
// Older CLIs do not always mount a fresh procfs. If this combination is blocked,
// it must report the specific incompatibility, rather than an unrelated error.
if(!protectedProc.ok)assert.match(protectedProc.stderr,/sandboxProcMountBlocked/);
const missingMetadata=await run(true,false,"missing");assert.equal(missingMetadata.ok,false);assert.match(missingMetadata.stderr,/sandboxMetadataRootMissing/);
const allowed=await run(true,false);assert.equal(allowed.ok,true,allowed.stderr);assert.match(allowed.stdout,/ISOLATION_OK/);
console.log(`PASS real Codex sandbox under full Dar hardening: netlink and missing metadata failures reproduced; protected proc ${protectedProc.ok?"supported":"blocked"}; corrected unit isolates the workspace.`);
