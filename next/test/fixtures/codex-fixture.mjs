#!/usr/bin/env node
// CLI fixture only. Real login/sandbox checks use the installed Codex binary separately.
import assert from "node:assert/strict";
import {readFile,writeFile} from "node:fs/promises";
import path from "node:path";
const args=process.argv.slice(2),[command,subcommand]=args,send=value=>process.stdout.write(JSON.stringify(value)+"\n");
assert.equal(process.env.OPENAI_API_KEY,undefined);assert.equal(process.env.CODEX_API_KEY,undefined);assert.equal(process.env.DAR_DATABASE_TOKEN_FILE,undefined);
if(command==="login"&&subcommand==="status"){
 process.stderr.write("Logged in using ChatGPT\n");
}else if(command==="debug"&&subcommand==="models"){
 send({models:[{slug:"fixture-model",display_name:"Fixture model",visibility:"list",priority:1}]});
}else if(command==="sandbox"){
 assert.ok(args.includes("-P")&&args.includes("dar"));
}else if(command==="exec"){
 assert.ok(args.includes("--json")&&args.includes("--ephemeral")&&args.includes("--ignore-user-config")&&args.includes("--ignore-rules"));
 assert.equal(args.at(-1),"-");assert.equal(args[args.indexOf("-m")+1],"fixture-model");
 assert.ok(args.includes('approval_policy="never"'));
 process.stdin.setEncoding("utf8");let prompt="";for await(const chunk of process.stdin)prompt+=chunk;
 send({type:"thread.started",thread_id:"fixture-thread"});send({type:"turn.started"});
 if(prompt.includes("WAIT_FOR_CANCEL")){
  send({type:"item.started",item:{id:"waiting-command",type:"command_execution",command:"fixture waiting for cancellation",status:"in_progress"}});setInterval(()=>{},1000);
 }else if(prompt.includes("FAIL_CLI")){
  send({type:"turn.failed",error:{message:"Fixture failure"}});process.exitCode=1;
 }else if(prompt.includes("EXIT_WITHOUT_TURN")){
  send({type:"item.completed",item:{id:"reply-1",type:"agent_message",text:"Unconfirmed response"}});
 }else{
  const cwd=args[args.indexOf("-C")+1],content=await readFile(path.join(cwd,"greeting.txt"),"utf8");
  await writeFile(path.join(cwd,"greeting.txt"),content.includes("Updated")?"Updated again by Codex fixture.\n":"Updated by Codex fixture.\n");
  send({type:"item.started",item:{id:"command-1",type:"command_execution",command:"fixture test command",status:"in_progress"}});
  send({type:"item.updated",item:{id:"command-1",type:"command_execution",aggregated_output:"fixture check "}});
  send({type:"item.completed",item:{id:"command-1",type:"command_execution",aggregated_output:"fixture check passed\n",exit_code:0,status:"completed"}});
  send({type:"item.updated",item:{id:"reply-1",type:"agent_message",text:"The edit "}});
  await new Promise(resolve=>setTimeout(resolve,800));
  send({type:"item.completed",item:{id:"reply-1",type:"agent_message",text:"The edit is ready."}});
  send({type:"turn.completed",usage:{input_tokens:10,output_tokens:5}});
 }
}else throw Error("Unexpected Codex CLI invocation: "+command);
