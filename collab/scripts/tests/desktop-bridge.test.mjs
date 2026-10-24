import test from 'node:test';
import assert from 'node:assert/strict';
import net from 'node:net';
import fs from 'node:fs/promises';
import fsSync from 'node:fs';
import path from 'node:path';
import os from 'node:os';
import { spawn } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const wrapper=process.env.COLLAB_TEST_DESKTOP_WRAPPER??fileURLToPath(new URL('../collab-desktop-mcp.mjs',import.meta.url));
function capabilityPath(state,cwd,session='agent-session',thread='real-thread'){
  let p=path.join(state,'runtime','desktop','capabilities','v1');
  for(const [label,value] of [['cwd',cwd],['session',session],['thread',thread]])p=path.join(p,label,...Buffer.from(value).toString('hex').match(/.{1,120}/g));
  return path.join(p,'current.json');
}
function encode(value){const body=Buffer.from(JSON.stringify(value));const head=Buffer.alloc(4);head.writeUInt32LE(body.length);return Buffer.concat([head,body]);}
function decode(stream,fn){let bytes=Buffer.alloc(0);stream.on('data',chunk=>{bytes=Buffer.concat([bytes,chunk]);while(bytes.length>=4){const n=bytes.readUInt32LE(0);assert.ok(n<=8*1024*1024);if(bytes.length<n+4)return;const v=JSON.parse(bytes.subarray(4,n+4));bytes=bytes.subarray(n+4);fn(v);}});}
async function fixture(t,{host=true,legacy=false,launcher=false,origin='Codex Desktop',stateRoot,nativeCwd,hostPipe}={}){
  const root=await fs.mkdtemp('/tmp/cdb-');
  const calls=[];const connections=new Set();
  const server=net.createServer(stream=>{connections.add(stream);stream.on('close',()=>connections.delete(stream));decode(stream,req=>{
    calls.push(req);
    if(req.method==='tools/list')stream.write(encode({id:req.id,result:{tools:[{name:'read_thread',namespace:'codex_app'},{name:'send_message_to_thread',namespace:'codex_app'}]}}));
    else if(req.params.tool==='read_thread')stream.write(encode({id:req.id,result:{success:true,contentItems:[{type:'inputText',text:JSON.stringify({schemaVersion:1,thread:{id:'real-thread',cwd:nativeCwd??root,status:{type:'idle'}},turns:[{id:'old-turn',status:'completed'}]})}]}}));
    else if(req.params.arguments.prompt==='DISCONNECT')stream.destroy();
    else if(req.params.arguments.prompt==='RPCERROR')stream.write(encode({id:req.id,error:{code:-32003,message:'native scope denied'}}));
    else stream.write(encode({id:req.id,result:{success:!req.params.arguments.prompt.includes('REJECT'),contentItems:[{type:'inputText',text:'native response'}]}}));
  });});
  await new Promise(resolve=>server.listen(path.join(root,'host.sock'),resolve));
  const cli=path.join(root,'fake-cli.mjs');
  await fs.writeFile(cli,`#!${process.execPath}\nimport fs from 'node:fs';const a=process.argv.slice(2);const i=a.indexOf('--provide');const f=i<0?{}:JSON.parse(a[i+1]);fs.writeFileSync(process.env.FIXTURE_ROOT+'/facts.json',JSON.stringify(f));console.log(JSON.stringify({ok:true,result:{outcome:'completed',snapshot:{facts:f}}}));\n`);
  await fs.chmod(cli,0o700);
  const env={...process.env,FIXTURE_ROOT:root,COLLAB_STATE_DIR:stateRoot??path.join(root,'state'),COLLAB_BIN:cli,COLLAB_MCP_BIN:process.env.COLLAB_TEST_MCP_BINARY??path.join(process.env.CARGO_HOME||path.join(os.homedir(),'.cargo'),'bin','collab-mcp'),CODEX_INTERNAL_ORIGINATOR_OVERRIDE:origin};
  delete env.CODEX_SESSION_ID;delete env.CODEX_THREAD_ID;delete env.CODEX_APP_TOOLS_PIPE_PATH;
  if(host)env.CODEX_APP_TOOLS_PIPE_PATH=hostPipe??path.join(root,'host.sock');
  env.CODEX_MCP_NODE_PATH=process.execPath;
  const child=legacy?spawn(env.COLLAB_MCP_BIN,[],{env,cwd:root}):launcher?spawn('/bin/sh',[path.join(path.dirname(wrapper),'collab-desktop-mcp')],{env,cwd:root}):spawn(process.execPath,[wrapper],{env,cwd:root});
  let stderr='';child.stderr.on('data',chunk=>stderr+=chunk);
  const pending=new Map();let seq=0,lines='';
  child.stdout.on('data',chunk=>{lines+=chunk;for(;;){const n=lines.indexOf('\n');if(n<0)return;const line=lines.slice(0,n);lines=lines.slice(n+1);if(!line.trim())continue;const value=JSON.parse(line);const p=pending.get(String(value.id));if(p){pending.delete(String(value.id));clearTimeout(p.timer);p.resolve(value);}}});
  child.on('exit',code=>{for(const p of pending.values()){clearTimeout(p.timer);p.reject(Error('MCP exited '+code+': '+stderr));}pending.clear();});
  // The harness must outlive the bridge's 5s native timeout so native failures
  // remain observable rather than racing the test's own deadline.
  function rpc(method,params){const id=String(++seq);return new Promise((resolve,reject)=>{const timer=setTimeout(()=>{pending.delete(id);reject(Error(`MCP timeout host=${host} origin=${origin} method=${method} tool=${params?.name}: ${stderr}`));},15000);pending.set(id,{resolve,reject,timer});child.stdin.write(JSON.stringify({jsonrpc:'2.0',id,method,params})+'\n');});}
  t.after(async()=>{child.stdin.end();if(child.exitCode===null&&child.signalCode===null)child.kill('SIGTERM');for(const stream of connections)stream.destroy();await new Promise(resolve=>server.close(resolve));await fs.rm(root,{recursive:true,force:true});});
  await rpc('initialize',{protocolVersion:'2024-11-05',capabilities:{},clientInfo:{name:'consumer',version:'1'}});
  const context=(extra={},meta=true)=>rpc('tools/call',{name:'collab_context',arguments:{project_scope:nativeCwd??root,provide:JSON.stringify({session_id:'agent-session',...extra})},...(meta?{_meta:{'x-codex-turn-metadata':{thread_id:'real-thread',turn_id:'real-origin-turn'}}}:{})});
  async function facts(){return JSON.parse(await fs.readFile(path.join(root,'facts.json'),'utf8'));}
  async function request(candidate,value){const stream=net.createConnection(candidate.endpoint.slice('unix://'.length));return await new Promise((resolve,reject)=>{const timer=setTimeout(()=>{stream.destroy();reject(Error('UDS timeout'));},5000);stream.on('error',error=>{clearTimeout(timer);reject(error);});stream.on('close',()=>{clearTimeout(timer);reject(Error('UDS closed before response'));});decode(stream,response=>{clearTimeout(timer);stream.end();resolve(response);});stream.on('connect',()=>stream.write(encode(value)));});}
  async function stopService(candidate){const service=await request(candidate,{id:'owned-service',op:'ping'});assert.ok(Number.isInteger(service.pid)&&service.pid>1);process.kill(service.pid,'SIGTERM');const deadline=Date.now()+5000;while(Date.now()<deadline){try{await fs.stat(candidate.endpoint.slice(7));}catch(e){if(e.code==='ENOENT')return;throw e;}await new Promise(r=>setTimeout(r,20));}throw Error('owned runtime service did not stop');}
  const serviceLogs=()=>{try{const dir=path.join(env.COLLAB_STATE_DIR,'runtime','desktop');return fsSync.readdirSync(dir).filter(n=>n.endsWith('.log')).map(n=>fsSync.readFileSync(path.join(dir,n),'utf8')).join('\n');}catch{return '';}};
  return {root,child,calls,context,facts,request,rpc,stopService,stderr:()=>stderr+serviceLogs()};
}

test('bridge restart rejects unsafe private enrollment without publishing or stopping other MCP tools',async t=>{
  const a=await fixture(t);await a.context();
  const state=path.join(a.root,'state'),pipe=path.join(a.root,'host.sock');
  const directory=path.join(state,'runtime','desktop','enrollments');
  const [record]=await fs.readdir(directory);
  const privateFile=path.join(directory,record);
  assert.equal((await fs.stat(privateFile)).mode&0o077,0);
  assert.equal(JSON.parse(await fs.readFile(privateFile,'utf8')).schema,'collab-desktop-enrollment/v2');
  assert.equal(JSON.parse(await fs.readFile(privateFile,'utf8')).binding.callerTurn,undefined);
  const current=capabilityPath(state,await fs.realpath(a.root));
  const prior=await fs.readFile(current,'utf8');
  await fs.chmod(privateFile,0o644);
  await a.stopService((await a.facts()).desktop_bridge);
  const stopped=new Promise(resolve=>a.child.once('exit',resolve));a.child.stdin.end();await stopped;
  const b=await fixture(t,{stateRoot:state,hostPipe:pipe});
  const deadline=Date.now()+5000;
  while(!b.stderr().includes('ENROLLMENT_UNSAFE')&&Date.now()<deadline)await new Promise(resolve=>setTimeout(resolve,20));
  assert.match(b.stderr(),/ENROLLMENT_UNSAFE/);
  assert.equal(await fs.readFile(current,'utf8'),prior);
  assert.ok((await b.rpc('tools/list',{})).result.tools.length>0);
});

test('same host restores enrolled bridge targets after process restart without a context call',async t=>{
  const a=await fixture(t);await a.context();
  const cwd=await fs.realpath(a.root),state=path.join(a.root,'state');
  const current=capabilityPath(state,cwd);
  const first=JSON.parse(await fs.readFile(current,'utf8')).desktop_bridge;
  await a.stopService(first);
  const stopped=new Promise(resolve=>a.child.once('exit',resolve));a.child.stdin.end();await stopped;
  const b=await fixture(t,{stateRoot:state,nativeCwd:cwd,hostPipe:path.join(a.root,'host.sock')});
  let replacement;
  const deadline=Date.now()+7000;
  while(Date.now()<deadline){
    replacement=JSON.parse(await fs.readFile(current,'utf8')).desktop_bridge;
    if(b.calls.some(c=>c.method==='tools/call')||a.calls.filter(c=>c.params?.tool==='read_thread').length>=2)break;
    await new Promise(resolve=>setTimeout(resolve,20));
  }
  assert.equal(replacement.endpoint,first.endpoint,'runtime endpoint remains stable across service restart');
  await assert.rejects(fs.readFile(path.join(b.root,'facts.json')),'no forwarded context command');
  const result=await b.request(replacement,{id:'restored',op:'send',session_id:'agent-session',thread_id:'real-thread',text:'after restart'});
  assert.equal(result.ok,true);
  const call=a.calls.findLast(c=>c.params?.arguments?.prompt==='after restart');
  assert.equal(call.params.threadId,'real-thread');
  assert.match(call.params.turnId,/^collab-service-correlate-/,'service correlation does not claim an agent turn');
});

test('host pipe replacement restores a verified enrollment without a context call',async t=>{
  const a=await fixture(t);await a.context();
  const cwd=await fs.realpath(a.root),state=path.join(a.root,'state'),current=capabilityPath(state,cwd);
  const first=JSON.parse(await fs.readFile(current,'utf8')).desktop_bridge;
  await a.stopService(first);
  const stopped=new Promise(resolve=>a.child.once('exit',resolve));a.child.stdin.end();await stopped;
  // A new host pipe must re-verify and republish the same persisted scope.
  const b=await fixture(t,{stateRoot:state,nativeCwd:cwd});
  let replacement;
  const deadline=Date.now()+7000;
  while(Date.now()<deadline){
    replacement=JSON.parse(await fs.readFile(current,'utf8')).desktop_bridge;
    if(replacement.endpoint!==first.endpoint)break;
    await new Promise(resolve=>setTimeout(resolve,20));
  }
  assert.notEqual(replacement.endpoint,first.endpoint);
  await assert.rejects(fs.readFile(path.join(b.root,'facts.json')),'no forwarded context command');
  assert.ok(b.calls.some(c=>c.params?.tool==='read_thread'&&c.params?.threadId==='real-thread'),'restore re-verifies the native thread');
  const sent=await b.request(replacement,{id:'pipe2',op:'send',session_id:'agent-session',thread_id:'real-thread',text:'after pipe switch'});
  assert.equal(sent.ok,true);
});

test('host pipe replacement refuses an enrollment whose native project differs',async t=>{
  const a=await fixture(t);await a.context();
  const cwd=await fs.realpath(a.root),state=path.join(a.root,'state'),current=capabilityPath(state,cwd);
  const first=JSON.parse(await fs.readFile(current,'utf8')).desktop_bridge;
  await a.stopService(first);
  const stopped=new Promise(resolve=>a.child.once('exit',resolve));a.child.stdin.end();await stopped;
  const b=await fixture(t,{stateRoot:state,nativeCwd:state});
  const deadline=Date.now()+5000;
  while(!b.stderr().includes('SCOPE_MISMATCH')&&Date.now()<deadline)await new Promise(resolve=>setTimeout(resolve,20));
  assert.match(b.stderr(),/SCOPE_MISMATCH/);
  assert.equal(JSON.parse(await fs.readFile(current,'utf8')).desktop_bridge.endpoint,first.endpoint);
  assert.ok((await b.rpc('tools/list',{})).result.tools.length>0,'rejected restore does not stop unrelated MCP tools');
});

test('a live competing publisher makes automatic restore explicitly ambiguous',async t=>{
  const a=await fixture(t);await a.context();
  const cwd=await fs.realpath(a.root),state=path.join(a.root,'state'),current=capabilityPath(state,cwd);
  const first=JSON.parse(await fs.readFile(current,'utf8')).desktop_bridge;
  const b=await fixture(t,{stateRoot:state,nativeCwd:cwd});
  const deadline=Date.now()+5000;
  while(!b.stderr().includes('SCOPE_AMBIGUOUS')&&Date.now()<deadline)await new Promise(resolve=>setTimeout(resolve,20));
  assert.match(b.stderr(),/SCOPE_AMBIGUOUS/);
  assert.equal(JSON.parse(await fs.readFile(current,'utf8')).desktop_bridge.endpoint,first.endpoint);
  assert.ok((await b.rpc('tools/list',{})).result.tools.length>0,'ambiguity does not stop unrelated MCP tools');
});

test('genuine enrollment publishes one current native scope and retirement leaves only inactive current metadata',async t=>{
  const f=await fixture(t);
  const cwd=await fs.realpath(f.root);
  const current=capabilityPath(path.join(f.root,'state'),cwd);
  await assert.rejects(fs.readFile(current));
  await f.context();
  const record=JSON.parse(await fs.readFile(current,'utf8'));
  assert.equal(record.schema,'collab-desktop-capability/v1');
  assert.equal(record.desktop_bridge.cwd,cwd);
  assert.equal(record.desktop_bridge.thread_id,'real-thread');
  assert.equal(record.desktop_bridge.session_id,'agent-session');
  assert.equal((await fs.stat(current)).mode&0o077,0);
  assert.ok(!JSON.stringify(record).includes('real-origin-turn'));
  await f.context();
  assert.equal((await fs.readdir(path.dirname(current))).length,2,'repeated context must reuse its owner file');
  const endpoint=record.desktop_bridge.endpoint;
  const exit=new Promise(resolve=>f.child.once('exit',resolve));
  f.child.stdin.end();await exit;
  assert.equal((await f.request(record.desktop_bridge,{id:'after-wrapper',op:'probe',session_id:'agent-session',thread_id:'real-thread'})).ok,true);
  await f.stopService(record.desktop_bridge);
  await assert.rejects(fs.stat(endpoint.slice(7)));
  assert.equal(JSON.parse(await fs.readFile(current,'utf8')).desktop_bridge.endpoint,endpoint);
  assert.deepEqual((await fs.readdir(path.dirname(current))).filter(n=>n!=='current.json'),[]);
});

test('old publisher retirement cannot delete its replacement and inactive metadata cannot block a new publisher',async t=>{
  const a=await fixture(t);await a.context();
  const cwd=await fs.realpath(a.root),state=path.join(a.root,'state'),current=capabilityPath(state,cwd);
  const first=(await a.facts()).desktop_bridge,services=path.join(state,'runtime','desktop','services');
  const b=await fixture(t,{stateRoot:state,nativeCwd:cwd});await b.context();
  const replacement=JSON.parse(await fs.readFile(current,'utf8')).desktop_bridge;
  assert.notEqual(replacement.endpoint,first.endpoint);
  const descriptors=async()=>Promise.all((await fs.readdir(services)).filter(n=>n.endsWith('.json')).map(async n=>JSON.parse(await fs.readFile(path.join(services,n),'utf8')).endpoint));
  assert.deepEqual((await descriptors()).sort(),[first.endpoint,replacement.endpoint].sort());
  await a.stopService(first);
  const exitA=new Promise(resolve=>a.child.once('exit',resolve));a.child.stdin.end();await exitA;
  assert.deepEqual(JSON.parse(await fs.readFile(current,'utf8')).desktop_bridge,replacement);
  assert.deepEqual(await descriptors(),[replacement.endpoint],'old publisher cleanup leaves the successor descriptor');
  assert.equal((await b.request(replacement,{id:'replacement',op:'probe',session_id:'agent-session',thread_id:'real-thread'})).ok,true);
  const exitB=new Promise(resolve=>b.child.once('exit',resolve));b.child.stdin.end();await exitB;
  const c=await fixture(t,{stateRoot:state,nativeCwd:cwd});await c.context();
  const latest=JSON.parse(await fs.readFile(current,'utf8')).desktop_bridge;
  assert.notEqual(latest.endpoint,replacement.endpoint);
  assert.equal((await c.request(latest,{id:'latest',op:'probe',session_id:'agent-session',thread_id:'real-thread'})).ok,true);
});

test('publication failure preserves the prior complete current capability',async t=>{
  const f=await fixture(t);await f.context();
  const current=capabilityPath(path.join(f.root,'state'),await fs.realpath(f.root));
  const before=await fs.readFile(current,'utf8'),directory=path.dirname(current);
  await fs.chmod(directory,0o500);
  let result;
  try{result=await f.context();}finally{await fs.chmod(directory,0o700);}
  assert.equal(result.result.isError,true);
  assert.match(result.result.content[0].text,/DESKTOP_BRIDGE_PUBLICATION_FAILED/);
  assert.equal(await fs.readFile(current,'utf8'),before);
  await f.context();
  assert.equal((await fs.readdir(directory)).length,2);
});

test('executor thread enrolls an independent agent session; scoped calls use explicit service correlation',async t=>{
  const f=await fixture(t,{legacy:process.env.DESKTOP_BRIDGE_LEGACY==='1'});
  await f.context({thread_id:'spoofed-thread',endpoint:'unix:///old-appserver.sock'});
  const facts=await f.facts();
  assert.equal(facts.thread_id,'real-thread');
  assert.equal(facts.session_id,'agent-session');
  assert.equal(facts.desktop_bridge.thread_id,'real-thread');
  assert.equal(facts.endpoint,undefined);
  const c=facts.desktop_bridge;
  const probe=await f.request(c,{id:'probe',op:'probe',session_id:'agent-session',thread_id:'real-thread'});
  assert.equal(probe.ok,true);assert.equal(probe.native.success,true);
  const sent=await f.request(c,{id:'send',op:'send',session_id:'agent-session',thread_id:'real-thread',text:'ACCEPT'});
  assert.equal(sent.ok,true);assert.equal(sent.native.success,true);
  const call=f.calls.find(r=>r.params?.tool==='send_message_to_thread');
  assert.equal(call.params.threadId,'real-thread');assert.match(call.params.turnId,/^collab-service-correlate-/);
  assert.equal(call.params.arguments.threadId,'real-thread');
  assert.equal(call.params.namespace,'codex_app');
  const before=f.calls.length;
  for(const v of [{id:'bad',op:'send',session_id:'other',thread_id:'real-thread',text:'X'},{id:'bad',op:'send',session_id:'agent-session',thread_id:'other',text:'X'},{id:'bad',op:'send',session_id:'agent-session',thread_id:'real-thread',text:'X',caller:'spoof'},{op:'send',session_id:'agent-session',thread_id:'real-thread',text:'X'}])assert.equal((await f.request(c,v)).ok,false);
  assert.equal(f.calls.length,before,'out of scope requests must not reach the host');
  const rejected=await f.request(c,{id:'reject',op:'send',session_id:'agent-session',thread_id:'real-thread',text:'REJECT'});
  assert.equal(rejected.ok,false);assert.equal(rejected.native.success,false);
  const rpcError=await f.request(c,{id:'native-error',op:'send',session_id:'agent-session',thread_id:'real-thread',text:'RPCERROR'});
  assert.equal(rpcError.ok,false);assert.deepEqual(rpcError.error,{code:-32003,message:'native scope denied'});
});

test('missing executor metadata cannot enroll a target; absent host remains a plain MCP proxy',async t=>{
  const f=await fixture(t);
  const rejected=await f.context({desktop_bridge:{thread_id:'fake'},turn_id:'fake'},false);
  assert.equal(rejected.result?.isError,true);
  await assert.rejects(fs.readFile(path.join(f.root,'facts.json')));
  const plain=await fixture(t,{host:false});
  await plain.context({},false);
  assert.equal((await plain.facts()).desktop_bridge,undefined);
  const tui=await fixture(t,{origin:'Codex CLI'});
  await tui.context();assert.equal((await tui.facts()).desktop_bridge,undefined);
});

test('host disconnect after a send never becomes success and ends the owned service',async t=>{
  const f=await fixture(t);await f.context();const c=(await f.facts()).desktop_bridge;
  await assert.rejects(f.request(c,{id:'unknown',op:'send',session_id:'agent-session',thread_id:'real-thread',text:'DISCONNECT'}),/closed|ECONNRESET/);
  const deadline=Date.now()+3000;while(Date.now()<deadline){try{await fs.stat(c.endpoint.slice(7));}catch{break;}await new Promise(r=>setTimeout(r,20));}
  assert.ok((await f.rpc('tools/list',{})).result.tools.length>0,'host failure does not stop unrelated MCP tools');
  assert.equal(f.calls.filter(r=>r.params?.tool==='send_message_to_thread').length,1,'there is no automatic retry');
  await assert.rejects(fs.stat(c.endpoint.slice('unix://'.length)));
});

test('normal wrapper stdin close and SIGTERM preserve the runtime-shared service',async t=>{
  for(const signal of [null,'SIGTERM']){
    const f=await fixture(t);await f.context();const c=(await f.facts()).desktop_bridge;
    const socket=c.endpoint.slice('unix://'.length);await fs.stat(socket);
    const exited=new Promise(resolve=>f.child.once('exit',resolve));
    if(signal)f.child.kill(signal);else f.child.stdin.end();
    await Promise.race([exited,new Promise((_,reject)=>setTimeout(()=>reject(Error('wrapper did not stop')),3000))]);
    await fs.stat(socket);
    assert.equal((await f.request(c,{id:'still-running',op:'probe',session_id:'agent-session',thread_id:'real-thread'})).ok,true);
  }
});

test('ordinary MCP frames retain integer request IDs without JavaScript number rounding',async t=>{
  const f=await fixture(t);
  const raw=new Promise((resolve,reject)=>{
    let text='';
    const timer=setTimeout(()=>reject(Error('raw MCP response timeout')),3000);
    function receive(chunk){text+=chunk;if(text.includes('\n')){clearTimeout(timer);f.child.stdout.off('data',receive);resolve(text);}}
    f.child.stdout.on('data',receive);
  });
  f.child.stdin.write('{"jsonrpc":"2.0","id":9007199254740993,"method":"ping"}\n');
  assert.match(await raw,/"id":9007199254740993/);
  for(const meta of [true,false]){
    const output=new Promise((resolve,reject)=>{
      let text='';const timer=setTimeout(()=>reject(Error('context response timeout')),5000);
      function receive(chunk){text+=chunk;if(text.includes('\n')){clearTimeout(timer);f.child.stdout.off('data',receive);resolve(text);}}
      f.child.stdout.on('data',receive);
    });
    const params={name:'collab_context',arguments:{provide:{session_id:'agent-session'}},...(meta?{_meta:{'x-codex-turn-metadata':{thread_id:'real-thread',turn_id:'real-origin-turn'}}}:{})};
    f.child.stdin.write('{"jsonrpc":"2.0","params":'+JSON.stringify(params)+',"id":9007199254740993,"method":"tools/call"}\n');
    const response=await output;
    assert.match(response,/"id":9007199254740993/);
    assert.equal(JSON.parse(response).result.isError,!meta);
  }
});

test('context accepts object facts and project_scope while retained queries stay pure',async t=>{
  const f=await fixture(t,{launcher:true});
  const result=await f.rpc('tools/call',{name:'collab_context',arguments:{project_scope:f.root,provide:{session_id:'agent-session'}},
    _meta:{'x-codex-turn-metadata':{thread_id:'real-thread',turn_id:'real-origin-turn'}}});
  assert.equal(result.result.isError,false);
  assert.equal((await f.facts()).desktop_bridge.cwd,await fs.realpath(f.root));
  const calls=f.calls.length;
  const queried=await f.rpc('tools/call',{name:'collab_context',arguments:{operation_id:'retained-operation',query:true}});
  assert.equal(queried.result.isError,false);
  assert.deepEqual(await f.facts(),{});
  assert.equal(f.calls.length,calls,'query must not probe or enroll');
});
