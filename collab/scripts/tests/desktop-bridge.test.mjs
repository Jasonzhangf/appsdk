import test from 'node:test';
import assert from 'node:assert/strict';
import net from 'node:net';
import fs from 'node:fs/promises';
import path from 'node:path';
import os from 'node:os';
import { spawn } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const wrapper=fileURLToPath(new URL('../collab-desktop-mcp.mjs',import.meta.url));
function encode(value){const body=Buffer.from(JSON.stringify(value));const head=Buffer.alloc(4);head.writeUInt32LE(body.length);return Buffer.concat([head,body]);}
function decode(stream,fn){let bytes=Buffer.alloc(0);stream.on('data',chunk=>{bytes=Buffer.concat([bytes,chunk]);while(bytes.length>=4){const n=bytes.readUInt32LE(0);assert.ok(n<=8*1024*1024);if(bytes.length<n+4)return;const v=JSON.parse(bytes.subarray(4,n+4));bytes=bytes.subarray(n+4);fn(v);}});}
async function fixture(t,{host=true,legacy=false,launcher=false,origin='Codex Desktop'}={}){
  const root=await fs.mkdtemp('/tmp/cdb-');
  const calls=[];const connections=new Set();
  const server=net.createServer(stream=>{connections.add(stream);stream.on('close',()=>connections.delete(stream));decode(stream,req=>{
    calls.push(req);
    if(req.method==='tools/list')stream.write(encode({id:req.id,result:{tools:[{name:'read_thread',namespace:'codex_app'},{name:'send_message_to_thread',namespace:'codex_app'}]}}));
    else if(req.params.tool==='read_thread')stream.write(encode({id:req.id,result:{success:true,contentItems:[{type:'inputText',text:JSON.stringify({schemaVersion:1,thread:{id:'real-thread',cwd:root,status:{type:'idle'}},turns:[{id:'old-turn',status:'completed'}]})}]}}));
    else if(req.params.arguments.prompt==='DISCONNECT')stream.destroy();
    else if(req.params.arguments.prompt==='RPCERROR')stream.write(encode({id:req.id,error:{code:-32003,message:'native scope denied'}}));
    else stream.write(encode({id:req.id,result:{success:!req.params.arguments.prompt.includes('REJECT'),contentItems:[{type:'inputText',text:'native response'}]}}));
  });});
  await new Promise(resolve=>server.listen(path.join(root,'host.sock'),resolve));
  const cli=path.join(root,'fake-cli.mjs');
  await fs.writeFile(cli,`#!${process.execPath}\nimport fs from 'node:fs';const a=process.argv.slice(2);const i=a.indexOf('--provide');const f=i<0?{}:JSON.parse(a[i+1]);fs.writeFileSync(process.env.FIXTURE_ROOT+'/facts.json',JSON.stringify(f));console.log(JSON.stringify({ok:true,result:{outcome:'completed',snapshot:{facts:f}}}));\n`);
  await fs.chmod(cli,0o700);
  const env={...process.env,FIXTURE_ROOT:root,COLLAB_STATE_DIR:path.join(root,'state'),COLLAB_BIN:cli,COLLAB_MCP_BIN:process.env.COLLAB_TEST_MCP_BINARY??path.join(process.env.CARGO_HOME||path.join(os.homedir(),'.cargo'),'bin','collab-mcp'),CODEX_INTERNAL_ORIGINATOR_OVERRIDE:origin};
  delete env.CODEX_SESSION_ID;delete env.CODEX_THREAD_ID;delete env.CODEX_APP_TOOLS_PIPE_PATH;
  if(host)env.CODEX_APP_TOOLS_PIPE_PATH=path.join(root,'host.sock');
  env.CODEX_MCP_NODE_PATH=process.execPath;
  const child=legacy?spawn(env.COLLAB_MCP_BIN,[],{env,cwd:root}):launcher?spawn('/bin/sh',[path.join(path.dirname(wrapper),'collab-desktop-mcp')],{env,cwd:root}):spawn(process.execPath,[wrapper],{env,cwd:root});
  let stderr='';child.stderr.on('data',chunk=>stderr+=chunk);
  const pending=new Map();let seq=0,lines='';
  child.stdout.on('data',chunk=>{lines+=chunk;for(;;){const n=lines.indexOf('\n');if(n<0)return;const line=lines.slice(0,n);lines=lines.slice(n+1);if(!line.trim())continue;const value=JSON.parse(line);const p=pending.get(String(value.id));if(p){pending.delete(String(value.id));clearTimeout(p.timer);p.resolve(value);}}});
  child.on('exit',code=>{for(const p of pending.values()){clearTimeout(p.timer);p.reject(Error('MCP exited '+code+': '+stderr));}pending.clear();});
  function rpc(method,params){const id=String(++seq);return new Promise((resolve,reject)=>{const timer=setTimeout(()=>{pending.delete(id);reject(Error('MCP timeout: '+stderr));},5000);pending.set(id,{resolve,reject,timer});child.stdin.write(JSON.stringify({jsonrpc:'2.0',id,method,params})+'\n');});}
  t.after(async()=>{child.stdin.end();if(child.exitCode===null&&child.signalCode===null)child.kill('SIGTERM');for(const stream of connections)stream.destroy();await new Promise(resolve=>server.close(resolve));await fs.rm(root,{recursive:true,force:true});});
  await rpc('initialize',{protocolVersion:'2024-11-05',capabilities:{},clientInfo:{name:'consumer',version:'1'}});
  const context=(extra={},meta=true)=>rpc('tools/call',{name:'collab_context',arguments:{provide:JSON.stringify({session_id:'agent-session',...extra})},...(meta?{_meta:{'x-codex-turn-metadata':{thread_id:'real-thread',turn_id:'real-origin-turn'}}}:{})});
  async function facts(){return JSON.parse(await fs.readFile(path.join(root,'facts.json'),'utf8'));}
  async function request(candidate,value){const stream=net.createConnection(candidate.endpoint.slice('unix://'.length));return await new Promise((resolve,reject)=>{const timer=setTimeout(()=>{stream.destroy();reject(Error('UDS timeout'));},5000);stream.on('error',error=>{clearTimeout(timer);reject(error);});stream.on('close',()=>{clearTimeout(timer);reject(Error('UDS closed before response'));});decode(stream,response=>{clearTimeout(timer);stream.end();resolve(response);});stream.on('connect',()=>stream.write(encode(value)));});}
  return {root,child,calls,context,facts,request,rpc};
}

test('executor metadata enrolls an independent agent session; scoped native calls retain the real caller',async t=>{
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
  assert.equal(call.params.threadId,'real-thread');assert.equal(call.params.turnId,'real-origin-turn');
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
  const exited=new Promise(resolve=>f.child.once('exit',resolve));
  await assert.rejects(f.request(c,{id:'unknown',op:'send',session_id:'agent-session',thread_id:'real-thread',text:'DISCONNECT'}),/closed|ECONNRESET/);
  await exited;
  assert.equal(f.calls.filter(r=>r.params?.tool==='send_message_to_thread').length,1,'there is no automatic retry');
  await assert.rejects(fs.stat(c.endpoint.slice('unix://'.length)));
});

test('normal stdin close and SIGTERM remove only the owned socket and stop the MCP child',async t=>{
  for(const signal of [null,'SIGTERM']){
    const f=await fixture(t);await f.context();const c=(await f.facts()).desktop_bridge;
    const socket=c.endpoint.slice('unix://'.length);await fs.stat(socket);
    const exited=new Promise(resolve=>f.child.once('exit',resolve));
    if(signal)f.child.kill(signal);else f.child.stdin.end();
    await Promise.race([exited,new Promise((_,reject)=>setTimeout(()=>reject(Error('wrapper did not stop')),3000))]);
    await assert.rejects(fs.stat(socket));
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
  assert.equal((await f.facts()).desktop_bridge.cwd,f.root);
  const calls=f.calls.length;
  const queried=await f.rpc('tools/call',{name:'collab_context',arguments:{operation_id:'retained-operation',query:true}});
  assert.equal(queried.result.isError,false);
  assert.deepEqual(await f.facts(),{});
  assert.equal(f.calls.length,calls,'query must not probe or enroll');
});
