#!/usr/bin/env node
// CLI fixture only. Real login/sandbox checks use the installed Codex binary separately.
import assert from "node:assert/strict";
import {readFile,writeFile,access} from "node:fs/promises";
import path from "node:path";
const args=process.argv.slice(2),[command,subcommand]=args,send=value=>process.stdout.write(JSON.stringify(value)+"\n");
assert.equal(process.env.OPENAI_API_KEY,undefined);assert.equal(process.env.CODEX_API_KEY,undefined);assert.equal(process.env.DAR_DATABASE_TOKEN_FILE,undefined);
if(command==="login"&&subcommand==="status"){
 const loggedOut=await access(path.join(process.env.CODEX_HOME,"logged-out")).then(()=>true,()=>false);
 process.stderr.write(loggedOut?"Not logged in\n":"Logged in using ChatGPT\n");if(loggedOut)process.exitCode=1;
}else if(command==="debug"&&subcommand==="models"){
 send({models:[{slug:"fixture-model",display_name:"Fixture model",visibility:"list",priority:1,default_reasoning_level:"medium",supported_reasoning_levels:[{effort:"low"},{effort:"medium"},{effort:"high"}]}]});
}else if(command==="sandbox"){
 assert.ok(args.includes("-P")&&args.includes("dar"));
}else if(command==="app-server"){
 // Minimal app-server: newline-delimited JSON-RPC with the request/notification shapes Dar uses.
 assert.ok(args.includes('approval_policy="never"'));assert.ok(args.includes('default_permissions="dar"'));assert.ok(args.includes("notify=[]"));
 const sessionsFile=path.join(process.env.CODEX_HOME,"fixture-sessions.json"),sessions=JSON.parse(await readFile(sessionsFile,"utf8").catch(()=>"{}"));
 const save=()=>writeFile(sessionsFile,JSON.stringify(sessions));
 let next=1,turn=0,cwd=process.cwd(),interrupted,steered,current;const replies=new Map(),shells=new Map();
 const notify=(method,params)=>send({method,params});
 const request=(method,params)=>new Promise(resolve=>{const id="server-"+next++;replies.set(id,resolve);send({id,method,params});});
 const wait=ms=>new Promise(resolve=>setTimeout(resolve,ms));
 const complete=(threadId,turnId,status="completed",error)=>notify("turn/completed",{threadId,turn:{id:turnId,status,items:[],...error?{error:{message:error}}:{}}});
 const message=(threadId,turnId,id,text)=>notify("item/completed",{threadId,turnId,item:{type:"agentMessage",id,text}});
 async function scenario(params,turnId){
  const threadId=params.threadId,text=params.input.find(item=>item.type==="text")?.text??"",images=params.input.filter(item=>item.type==="localImage");
  sessions[threadId].push(text);await save();
  if(text.includes("WAIT_FOR_CANCEL")){notify("item/started",{threadId,turnId,item:{type:"commandExecution",id:"waiting-command",command:"fixture waiting for cancellation",status:"inProgress"}});await new Promise(resolve=>{interrupted=resolve;});complete(threadId,turnId,"interrupted");return;}
  if(text.includes("FAIL_CLI")){complete(threadId,turnId,"failed","Fixture failure");return;}
  if(text.includes("EXIT_WITHOUT_TURN")){message(threadId,turnId,"reply-1","Unconfirmed response");await wait(50);process.exit(0);}
  if(text.includes("ASK_QUESTION")){const answer=await request("item/tool/requestUserInput",{threadId,turnId,itemId:"question-1",isBlocking:true,autoResolutionMs:null,questions:[{id:"color",header:"Color",question:"Which color should the greeting use?",isOther:true,isSecret:false,options:[{label:"Blue",description:"Calm"},{label:"Red",description:"Bold"}]}]});message(threadId,turnId,"reply-1",`You chose ${answer.answers.color.answers.join(", ")}.`);complete(threadId,turnId);return;}
  if(text.includes("REQUEST_APPROVAL")){const decision=await request("item/commandExecution/requestApproval",{threadId,turnId,itemId:"approval-1",kind:"command",startedAtMs:Date.now(),environmentId:null,command:"curl https://example.test",reason:"Needs network access"});assert.equal(decision.decision,"decline");message(threadId,turnId,"reply-1","Approval was declined.");complete(threadId,turnId);return;}
  if(text.includes("WAIT_FOR_STEER")){notify("item/started",{threadId,turnId,item:{type:"commandExecution",id:"steer-wait",command:"fixture waiting for guidance",status:"inProgress"}});const guidance=await new Promise(resolve=>{steered=resolve;});notify("item/completed",{threadId,turnId,item:{type:"commandExecution",id:"steer-wait",command:"fixture waiting for guidance",status:"completed",exitCode:0,aggregatedOutput:""}});message(threadId,turnId,"reply-1",`Steered: ${guidance}`);complete(threadId,turnId);return;}
  if(text.includes("PLAN_ONLY")){assert.equal(params.collaborationMode?.mode,"plan");message(threadId,turnId,"reply-1","Planned without editing.");complete(threadId,turnId);return;}
  if(text.includes("DESCRIBE_IMAGE")){for(const image of images){const bytes=await readFile(image.path);assert.equal(bytes.subarray(1,4).toString(),"PNG");}message(threadId,turnId,"reply-1",`Saw ${images.length} image${images.length===1?"":"s"}.`);complete(threadId,turnId);return;}
  if(text.includes("RESUME_CHECK")){message(threadId,turnId,"reply-1",`Turns: ${sessions[threadId].length}. Input: ${text.replace(/\s+/g," ").trim()}`);complete(threadId,turnId);return;}
  const content=await readFile(path.join(cwd,"greeting.txt"),"utf8");
  await writeFile(path.join(cwd,"greeting.txt"),content.includes("Updated")?"Updated again by Codex fixture.\n":"Updated by Codex fixture.\n");
  notify("item/completed",{threadId,turnId,item:{type:"reasoning",id:"reasoning-1",summary:["Checking the greeting file before editing."],content:[]}});
  notify("turn/plan/updated",{threadId,turnId,explanation:null,plan:[{step:"Read greeting",status:"completed"},{step:"Update greeting",status:"pending"}]});
  notify("item/completed",{threadId,turnId,item:{type:"fileChange",id:"edit-1",status:"completed",changes:[{path:"greeting.txt",kind:{type:"update"},diff:""}]}});
  notify("item/started",{threadId,turnId,item:{type:"commandExecution",id:"command-1",command:"fixture test command",status:"inProgress"}});
  notify("item/commandExecution/outputDelta",{threadId,turnId,itemId:"command-1",delta:"fixture check "});
  notify("item/completed",{threadId,turnId,item:{type:"commandExecution",id:"command-1",command:"fixture test command",status:"completed",exitCode:0,aggregatedOutput:"fixture check passed\n"}});
  notify("item/agentMessage/delta",{threadId,turnId,itemId:"reply-1",delta:"The edit "});await wait(800);
  message(threadId,turnId,"reply-1","The edit is ready.");
  const usage={totalTokens:15,inputTokens:10,cachedInputTokens:4,cacheWriteInputTokens:0,outputTokens:5,reasoningOutputTokens:0};
  notify("thread/tokenUsage/updated",{threadId,turnId,tokenUsage:{total:usage,last:usage,modelContextWindow:200000}});
  complete(threadId,turnId);
 }
 process.stdin.setEncoding("utf8");let buffer="";
 for await(const chunk of process.stdin){
  buffer+=chunk;let index;
  while((index=buffer.indexOf("\n"))>=0){
   const line=buffer.slice(0,index);buffer=buffer.slice(index+1);if(!line.trim())continue;
   const value=JSON.parse(line),{id,method,params}=value;
   if(!method){replies.get(id)?.(value.result);replies.delete(id);continue;}
   if(method==="initialize"){assert.equal(params.capabilities.experimentalApi,true);send({id,result:{userAgent:"fixture"}});}
   else if(method==="initialized"){}
   else if(method==="thread/start"){assert.equal(params.approvalPolicy,"never");assert.equal(params.model,"fixture-model");cwd=params.cwd??cwd;const threadId="fixture-thread-"+Object.keys(sessions).length;sessions[threadId]=[];await save();send({id,result:{thread:{id:threadId}}});}
   else if(method==="thread/resume"){if(!sessions[params.threadId])send({id,error:{code:-32600,message:"no such thread"}});else{cwd=params.cwd??cwd;send({id,result:{thread:{id:params.threadId}}});}}
   else if(method==="turn/start"){assert.equal(params.approvalPolicy,"never");if(params.effort)assert.match(params.effort,/^(low|medium|high)$/);const turnId="turn-"+ ++turn;current=turnId;send({id,result:{turn:{id:turnId,status:"inProgress",items:[]}}});scenario(params,turnId).catch(error=>{process.stderr.write(String(error));process.exit(3);});}
   else if(method==="turn/steer"){assert.equal(params.expectedTurnId,current);send({id,result:{turnId:current}});steered?.(params.input.find(item=>item.type==="text")?.text??"");}
   else if(method==="turn/interrupt"){send({id,result:{}});interrupted?.();}
   else if(method==="command/exec"){
    // A tiny line shell over the PTY protocol: echo, pwd and exit.
    assert.equal(params.tty,true);assert.equal(params.sandboxPolicy,undefined);const processId=params.processId;let line="";
    const out=text=>notify("command/exec/outputDelta",{processId,stream:"stdout",deltaBase64:Buffer.from(text).toString("base64"),capReached:false});
    shells.set(processId,{input(data){for(const char of data){if(char==="\r"){out("\r\n");const command=line.trim();line="";if(command==="exit"){shells.delete(processId);send({id,result:{exitCode:0,stdout:"",stderr:""}});return;}out(command==="pwd"?params.cwd+"\r\n":command.startsWith("echo ")?command.slice(5)+"\r\n":command?`sh: ${command}: not found\r\n`:"");out("$ ");}else if(char==="\x7f"){if(line){line=line.slice(0,-1);out("\b \b");}}else{line+=char;out(char);}}}});
    out("$ ");
   }
   else if(method==="command/exec/write"){shells.get(params.processId)?.input(Buffer.from(params.deltaBase64??"","base64").toString());send({id,result:{}});}
   else if(method==="command/exec/resize"){assert.ok(params.size.rows>0&&params.size.cols>0);send({id,result:{}});}
   else send({id,error:{code:-32601,message:"unsupported "+method}});
  }
 }
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
  const effort=args.find(value=>value.startsWith("model_reasoning_effort="));if(effort)assert.match(effort,/^model_reasoning_effort="(low|medium|high)"$/);
  send({type:"item.completed",item:{id:"reasoning-1",type:"reasoning",text:"Checking the greeting file before editing."}});
  send({type:"item.started",item:{id:"plan-1",type:"todo_list",items:[{text:"Read greeting",completed:true},{text:"Update greeting",completed:false}]}});
  send({type:"item.completed",item:{id:"edit-1",type:"file_change",status:"completed",changes:[{path:"greeting.txt",kind:"update"}]}});
  send({type:"item.started",item:{id:"command-1",type:"command_execution",command:"fixture test command",status:"in_progress"}});
  send({type:"item.updated",item:{id:"command-1",type:"command_execution",aggregated_output:"fixture check "}});
  send({type:"item.completed",item:{id:"command-1",type:"command_execution",aggregated_output:"fixture check passed\n",exit_code:0,status:"completed"}});
  send({type:"item.updated",item:{id:"reply-1",type:"agent_message",text:"The edit "}});
  await new Promise(resolve=>setTimeout(resolve,800));
  send({type:"item.completed",item:{id:"reply-1",type:"agent_message",text:"The edit is ready."}});
  send({type:"turn.completed",usage:{input_tokens:10,cached_input_tokens:4,output_tokens:5}});
 }
}else throw Error("Unexpected Codex CLI invocation: "+command);
