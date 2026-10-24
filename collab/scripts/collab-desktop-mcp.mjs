#!/usr/bin/env node
// Desktop-owned MCP entry. The host owns this foreground process and its pipe.
import net from 'node:net';
import fs from 'node:fs';
import path from 'node:path';
import os from 'node:os';
import { randomUUID } from 'node:crypto';
import { spawn } from 'node:child_process';

const MAX_FRAME=8*1024*1024;
const TIMEOUT_MS=5000;
const pipe=process.env.CODEX_APP_TOOLS_PIPE_PATH;
const origin=process.env.CODEX_INTERNAL_ORIGINATOR_OVERRIDE;
const desktop=!!pipe&&!['Codex CLI','Codex TUI'].includes(origin);
const stateRoot=process.env.COLLAB_STATE_DIR||path.join(os.homedir(),'.collab');
const runtimeDir=path.join(stateRoot,'runtime','desktop');
const socketPath=path.join(runtimeDir,`${process.pid}-${randomUUID().slice(0,8)}.sock`);
const cargoHome=process.env.CARGO_HOME||path.join(os.homedir(),'.cargo');
const child=spawn(process.env.COLLAB_MCP_BIN||path.join(cargoHome,'bin','collab-mcp'),[],{stdio:['pipe','pipe','pipe']});
const bindings=new Map();
const clients=new Set();
const pending=new Map();
const publicationFiles=new Set();
let sequence=0,stopping=false,ownedSocket=false,listener,upstream,tools,hostFailure;

function encoded(value){
  const body=Buffer.from(JSON.stringify(value));
  if(body.length>MAX_FRAME)throw Error('DESKTOP_BRIDGE_FRAME_TOO_LARGE');
  const header=Buffer.alloc(4);header.writeUInt32LE(body.length);
  return Buffer.concat([header,body]);
}
function readNative(stream,receive,fatal){
  let bytes=Buffer.alloc(0);
  stream.on('data',chunk=>{
    bytes=Buffer.concat([bytes,chunk]);
    try{
      while(bytes.length>=4){
        const length=bytes.readUInt32LE(0);
        if(length>MAX_FRAME)throw Error('DESKTOP_BRIDGE_FRAME_TOO_LARGE');
        if(bytes.length<length+4)return;
        const value=JSON.parse(bytes.subarray(4,length+4).toString('utf8'));
        bytes=bytes.subarray(length+4);receive(value);
      }
    }catch(error){fatal(error);}
  });
}
function hostCall(method,params){
  if(hostFailure||!upstream||stopping)return Promise.reject(hostFailure||Error('DESKTOP_BRIDGE_UNAVAILABLE'));
  const id=`bridge-${++sequence}`;
  return new Promise((resolve,reject)=>{
    const timer=setTimeout(()=>{pending.delete(id);reject(Error('DESKTOP_BRIDGE_RESULT_UNKNOWN: host response timeout'));},TIMEOUT_MS);
    pending.set(id,{resolve,reject,timer});
    try{upstream.write(encoded({jsonrpc:'2.0',id,method,params}));}
    catch(error){clearTimeout(timer);pending.delete(id);reject(error);}
  });
}
function nativeCall(binding,name,args){
  const tool=tools.get(name);
  if(!tool)throw Error(`DESKTOP_BRIDGE_TOOL_UNAVAILABLE: ${name}`);
  return hostCall('tools/call',{arguments:args,callerSource:'codex',callId:`collab-${randomUUID()}`,
    namespace:tool.namespace,threadId:binding.callerThread,turnId:binding.callerTurn,tool:name});
}
function key(session,thread){return JSON.stringify([session,thread]);}
async function handleLocal(request){
  const allowed=request?.op==='send'?['id','op','session_id','thread_id','text']:['id','op','session_id','thread_id'];
  if(!request||typeof request!=='object'||Object.keys(request).some(k=>!allowed.includes(k)))throw Error('DESKTOP_BRIDGE_INVALID_REQUEST');
  if(!['string','number'].includes(typeof request.id)||request.id==='')throw Error('DESKTOP_BRIDGE_REQUEST_ID_REQUIRED');
  if(!['probe','send'].includes(request.op))throw Error('DESKTOP_BRIDGE_OPERATION_UNSUPPORTED');
  const binding=bindings.get(key(request.session_id,request.thread_id));
  if(!binding)throw Error('DESKTOP_BRIDGE_SCOPE_MISMATCH: target is not enrolled by this executor');
  if(request.op==='send'&&typeof request.text!=='string')throw Error('DESKTOP_BRIDGE_INVALID_TEXT');
  const response=await nativeCall(binding,request.op==='probe'?'read_thread':'send_message_to_thread',request.op==='probe'
    ?{threadId:binding.thread,turnLimit:1,includeOutputs:false,maxOutputCharsPerItem:100}
    :{threadId:binding.thread,prompt:request.text});
  if(response.error)return {id:request.id,ok:false,error:response.error};
  const native=response.result;
  const result={id:request.id,ok:native?.success===true,session_id:binding.session,thread_id:binding.thread,native};
  if(!result.ok)result.error='DESKTOP_BRIDGE_NATIVE_REJECTED';
  return result;
}
function controlDirectory(target){
  fs.mkdirSync(stateRoot,{recursive:true,mode:0o700});
  let current=fs.realpathSync(stateRoot);
  const check=p=>{const stat=fs.lstatSync(p);if(!stat.isDirectory()||stat.isSymbolicLink()||stat.uid!==process.getuid()||(stat.mode&0o022))throw Error('DESKTOP_BRIDGE_DECLARATION_UNSAFE: control directory');};
  check(current);
  for(const component of path.relative(stateRoot,target).split(path.sep)){
    if(!component||component==='.'||component==='..')throw Error('DESKTOP_BRIDGE_DECLARATION_INVALID: control path');
    current=path.join(current,component);
    try{fs.mkdirSync(current,{mode:0o700});}catch(error){if(error.code!=='EEXIST')throw error;}
    check(current);
  }
  return current;
}
async function publishCapability(binding,expectedCwd){
  const response=await nativeCall(binding,'read_thread',{threadId:binding.thread,turnLimit:1,includeOutputs:false,maxOutputCharsPerItem:1});
  if(response.error)throw Error(JSON.stringify(response.error));
  const native=response.result;
  if(native?.success!==true)throw Error(`DESKTOP_BRIDGE_PUBLICATION_FAILED: ${JSON.stringify(native)}`);
  const content=native.contentItems?.find(item=>item.type==='inputText'&&typeof item.text==='string');
  const observed=content?JSON.parse(content.text):undefined;
  if(observed?.thread?.id!==binding.thread||typeof observed.thread.cwd!=='string')throw Error('DESKTOP_BRIDGE_PUBLICATION_FAILED: missing current native scope');
  const cwd=fs.realpathSync(observed.thread.cwd);
  if(expectedCwd!==undefined&&cwd!==fs.realpathSync(expectedCwd))throw Error('DESKTOP_BRIDGE_SCOPE_MISMATCH: enrolled project changed');
  if(stopping)throw Error('DESKTOP_BRIDGE_UNAVAILABLE: retiring');
  const candidate={endpoint:`unix://${socketPath}`,namespace:tools.get('send_message_to_thread').namespace,session_id:binding.session,thread_id:binding.thread,cwd};
  let directory=path.join(runtimeDir,'capabilities','v1');
  for(const [label,value] of [['cwd',cwd],['session',binding.session],['thread',binding.thread]])directory=path.join(directory,label,...Buffer.from(value).toString('hex').match(/.{1,120}/g));
  directory=controlDirectory(directory);
  const owner=path.join(directory,`${path.basename(socketPath)}.json`);
  const temporary=owner+'.tmp';
  let createdOwner=false;
  try{
    if(!publicationFiles.has(owner)){
      fs.writeFileSync(owner,JSON.stringify({schema:'collab-desktop-capability/v1',desktop_bridge:candidate}),{flag:'wx',mode:0o600});
      publicationFiles.add(owner);createdOwner=true;
    }
    fs.linkSync(owner,temporary);publicationFiles.add(temporary);
    fs.renameSync(temporary,path.join(directory,'current.json'));
    // POSIX rename is a no-op when both names already link the same inode.
    // Repeated publication must also remove that still-owned temporary link.
    removePublicationFile(temporary);
  }catch(error){
    if(publicationFiles.has(temporary))removePublicationFile(temporary);
    if(createdOwner)removePublicationFile(owner);
    throw Error(`DESKTOP_BRIDGE_PUBLICATION_FAILED: ${error.message}`);
  }
  return candidate;
}
function enrollmentDirectory(){
  // A host pipe is an execution authority. A different host must enroll its
  // own caller metadata; it cannot inherit a prior host's enrollment.
  return controlDirectory(path.join(runtimeDir,'enrollments',Buffer.from(pipe).toString('base64url')));
}
function persistEnrollment(binding,cwd){
  const directory=enrollmentDirectory();
  const file=path.join(directory,Buffer.from(key(binding.session,binding.thread)).toString('base64url')+'.json');
  const temporary=file+'.'+randomUUID()+'.tmp';
  try{
    fs.writeFileSync(temporary,JSON.stringify({schema:'collab-desktop-enrollment/v1',host_pipe:pipe,cwd,binding}),{flag:'wx',mode:0o600});
    fs.renameSync(temporary,file);
  }finally{
    try{fs.unlinkSync(temporary);}catch(error){if(error.code!=='ENOENT')throw error;}
  }
}
async function restoreEnrollments(){
  if(!desktop)return;
  const directory=enrollmentDirectory();
  await Promise.all(fs.readdirSync(directory).filter(name=>name.endsWith('.json')).map(async name=>{
    let binding;
    try{
      const file=path.join(directory,name),stat=fs.lstatSync(file);
      if(!stat.isFile()||stat.isSymbolicLink()||stat.uid!==process.getuid()||(stat.mode&0o077))throw Error('DESKTOP_BRIDGE_ENROLLMENT_UNSAFE');
      const record=JSON.parse(fs.readFileSync(file,'utf8'));
      binding=record.binding;
      if(record.schema!=='collab-desktop-enrollment/v1'||record.host_pipe!==pipe||typeof record.cwd!=='string'
        ||!binding||['session','thread','callerThread','callerTurn'].some(k=>typeof binding[k]!=='string'||!binding[k])
        ||binding.callerThread!==binding.thread)throw Error('DESKTOP_BRIDGE_ENROLLMENT_INVALID');
      // Reuse genuine enrolled caller metadata only after the current host
      // verifies the same native thread and project. No turn is synthesized.
      await publishCapability(binding,record.cwd);
      bindings.set(key(binding.session,binding.thread),binding);
    }catch(error){
      process.stderr.write(`Desktop bridge enrollment pending: ${error.message}\n`);
    }
  }));
}
function removePublicationFile(file){
  try{fs.unlinkSync(file);publicationFiles.delete(file);}catch(error){if(error.code==='ENOENT')publicationFiles.delete(file);else process.stderr.write(`Desktop publication cleanup: ${error.message}\n`);}
}
function removeSocket(){
  for(const file of publicationFiles)removePublicationFile(file);
  if(!ownedSocket)return;
  ownedSocket=false;
  try{fs.unlinkSync(socketPath);}catch(error){if(error.code!=='ENOENT')process.stderr.write(`Desktop bridge cleanup: ${error.message}\n`);}
}
function stop(code){
  if(stopping)return;
  stopping=true;
  for(const entry of pending.values()){clearTimeout(entry.timer);entry.reject(Error('DESKTOP_BRIDGE_RESULT_UNKNOWN: connection closed'));}
  pending.clear();
  for(const client of clients)client.destroy();
  listener?.close();upstream?.destroy();removeSocket();
  child.stdin.end();
  const terminate=setTimeout(()=>child.kill('SIGTERM'),1000);
  const bound=setTimeout(()=>process.exit(code),2000);
  child.once('close',()=>{clearTimeout(terminate);clearTimeout(bound);process.exit(code);});
  if(child.exitCode!==null||child.signalCode!==null){clearTimeout(terminate);clearTimeout(bound);process.exit(code);}
}
async function openHost(){
  if(!desktop)return;
  controlDirectory(runtimeDir);
  upstream=net.createConnection(pipe);
  readNative(upstream,response=>{
    const entry=pending.get(String(response.id));
    if(!entry)return;
    pending.delete(String(response.id));clearTimeout(entry.timer);entry.resolve(response);
  },error=>{hostFailure=error;stop(1);});
  upstream.on('error',error=>{hostFailure=error;});
  upstream.on('close',()=>{hostFailure??=Error('DESKTOP_BRIDGE_UNAVAILABLE: host closed');stop(1);});
  await new Promise((resolve,reject)=>{upstream.once('connect',resolve);upstream.once('error',reject);});
  const response=await hostCall('tools/list',{threadStartKind:'all'});
  if(response.error)throw Error(JSON.stringify(response.error));
  const advertised=response.result?.tools;
  if(!Array.isArray(advertised))throw Error('DESKTOP_BRIDGE_INVALID_CAPABILITIES');
  tools=new Map(advertised.filter(t=>typeof t.name==='string'&&typeof t.namespace==='string').map(t=>[t.name,t]));
  for(const name of ['read_thread','send_message_to_thread'])if(!tools.has(name))throw Error(`DESKTOP_BRIDGE_TOOL_UNAVAILABLE: ${name}`);
  listener=net.createServer(client=>{
    clients.add(client);client.on('close',()=>clients.delete(client));client.on('error',()=>{});
    readNative(client,request=>{
      void handleLocal(request).then(response=>{if(!client.destroyed)client.write(encoded(response));},error=>{
        if(!client.destroyed)client.write(encoded({id:request?.id??null,ok:false,error:error.message}));
      });
    },()=>client.destroy());
  });
  await new Promise((resolve,reject)=>{listener.once('error',reject);listener.listen(socketPath,()=>{ownedSocket=true;fs.chmodSync(socketPath,0o600);resolve();});});
}
const ready=openHost().then(restoreEnrollments).catch(error=>{hostFailure=error;throw error;});
// A host error must be observed even before the first context call.
ready.catch(()=>{});
function stringValue(object,keys){for(const k of keys){if(typeof object?.[k]==='string'&&object[k].trim())return object[k];}return undefined;}
function executorContext(meta){
  let turnMeta=meta?.['x-codex-turn-metadata'];
  if(typeof turnMeta==='string'){try{turnMeta=JSON.parse(turnMeta);}catch{turnMeta=undefined;}}
  return {thread:stringValue(meta,['openai/threadId','openai/thread_id','codexThreadId','codex_thread_id','threadId','thread_id'])||stringValue(turnMeta,['thread_id'])||stringValue(meta?.thread,['id']),
    turn:stringValue(meta,['openai/turnId','openai/turn_id','codexTurnId','codex_turn_id','turnId','turn_id'])||stringValue(turnMeta,['turn_id'])||stringValue(meta?.turn,['id'])};
}
function mcpFrame(body,kind){
  const bytes=Buffer.isBuffer(body)?body:Buffer.from(JSON.stringify(body));
  if(kind==='header')return Buffer.concat([Buffer.from(`Content-Length: ${bytes.length}\r\n\r\n`),bytes]);
  return Buffer.concat([bytes,Buffer.from('\n')]);
}
// Locate only the top-level ID token. Keep its original bytes when context
// arguments are rewritten; JSON.parse cannot represent every JSON integer.
function idSpan(text){
  let depth=0,span;
  for(let i=0;i<text.length;i++){
    const c=text[i];
    if(c==='{'||c==='['){depth++;continue;}
    if(c==='}'||c===']'){depth--;continue;}
    if(c!=='"')continue;
    const start=i++;
    while(i<text.length){if(text[i]==='\\'){i+=2;continue;}if(text[i]==='"')break;i++;}
    const end=i+1;
    if(depth!==1||JSON.parse(text.slice(start,end))!=='id')continue;
    let colon=end;while(/\s/.test(text[colon]||'')&&colon<text.length)colon++;
    if(text[colon]!==':')continue;
    let valueStart=colon+1;while(/\s/.test(text[valueStart]||'')&&valueStart<text.length)valueStart++;
    const token=/^(?:"(?:\\.|[^"\\])*"|null|-?(?:0|[1-9]\d*)(?:\.\d+)?(?:[eE][+-]?\d+)?)/.exec(text.slice(valueStart));
    if(token)span={start:valueStart,end:valueStart+token[0].length};
  }
  return span;
}
function rewrittenFrame(value,kind,rawId){
  let text=JSON.stringify(value);
  const span=idSpan(text);
  if(rawId!==undefined&&span)text=text.slice(0,span.start)+rawId+text.slice(span.end);
  return mcpFrame(Buffer.from(text),kind);
}
async function forward(body,kind){
  let request;
  try{request=JSON.parse(body.toString('utf8'));}catch{child.stdin.write(mcpFrame(body,kind));return;}
  if(!(desktop&&request.method==='tools/call'&&request.params?.name==='collab_context'&&request.params.arguments?.query!==true)){
    child.stdin.write(mcpFrame(body,kind));
    return;
  }
  if(desktop&&request.method==='tools/call'&&request.params?.name==='collab_context'){
    const original=body.toString('utf8'),span=idSpan(original);
    const rawId=span?original.slice(span.start,span.end):undefined;
    try{
      const context=executorContext(request.params._meta);
      if(!context.thread||!context.turn)throw Error('DESKTOP_BRIDGE_CONTEXT_REQUIRED: executor thread and turn metadata are missing');
      await ready;
      const args=request.params.arguments??{};
      const facts=args.provide===undefined?{}:typeof args.provide==='string'?JSON.parse(args.provide):args.provide;
      if(!facts||typeof facts!=='object'||Array.isArray(facts))throw Error('DESKTOP_BRIDGE_INVALID_FACTS');
      if(['caller','caller_thread_id','caller_turn_id','turn_id'].some(k=>k in facts))throw Error('DESKTOP_BRIDGE_INVALID_FACTS: caller context is executor-owned');
      const session=stringValue(facts,['session_id'])||stringValue(process.env,['CODEX_SESSION_ID']);
      if(!session)throw Error('DESKTOP_BRIDGE_SESSION_REQUIRED: provide the current agent session_id');
      const binding={session,thread:context.thread,callerThread:context.thread,callerTurn:context.turn};
      bindings.set(key(session,context.thread),binding);
      const candidate=await publishCapability(binding);
      persistEnrollment(binding,candidate.cwd);
      facts.session_id=session;facts.thread_id=context.thread;facts.desktop_bridge=candidate;
      delete facts.endpoint;delete facts.namespace;
      request.params.arguments={...args,provide:JSON.stringify(facts)};
    }catch(error){
      process.stdout.write(rewrittenFrame({jsonrpc:'2.0',id:request.id,result:{content:[{type:'text',text:error.message}],isError:true}},kind,rawId));
      return;
    }
    child.stdin.write(rewrittenFrame(request,kind,rawId));
    return;
  }
  child.stdin.write(mcpFrame(request,kind));
}
let input=Buffer.alloc(0),forwardQueue=Promise.resolve();
process.stdin.on('data',chunk=>{
  input=Buffer.concat([input,chunk]);
  try{
    while(input.length){
      let body,kind='line';
      if(input.subarray(0,15).toString().toLowerCase().startsWith('content-length:')){
        const end=input.indexOf('\r\n\r\n');if(end<0)return;
        const match=/^Content-Length:\s*(\d+)\r?$/im.exec(input.subarray(0,end).toString());
        if(!match)throw Error('invalid MCP frame');
        const n=Number(match[1]);if(n>MAX_FRAME)throw Error('MCP frame too large');
        if(input.length<end+4+n)return;
        body=input.subarray(end+4,end+4+n);input=input.subarray(end+4+n);kind='header';
      }else{
        const end=input.indexOf('\n');if(end<0){if(input.length>MAX_FRAME)throw Error('MCP frame too large');return;}
        body=input.subarray(0,end);input=input.subarray(end+1);if(body.length>MAX_FRAME)throw Error('MCP frame too large');if(!body.toString().trim())continue;
      }
      forwardQueue=forwardQueue.then(()=>forward(body,kind)).catch(error=>{process.stderr.write(`Desktop bridge proxy: ${error.message}\n`);stop(1);});
    }
  }catch(error){process.stderr.write(`${error.message}\n`);stop(1);}
});
child.stdout.pipe(process.stdout);
child.stderr.pipe(process.stderr);
child.on('error',error=>{process.stderr.write(`${error.message}\n`);stop(1);});
child.on('close',()=>{if(!stopping)stop(child.exitCode??1);});
process.stdin.on('end',()=>void forwardQueue.finally(()=>stop(0)));
process.on('SIGTERM',()=>stop(0));
process.on('SIGINT',()=>stop(0));
process.on('exit',removeSocket);
