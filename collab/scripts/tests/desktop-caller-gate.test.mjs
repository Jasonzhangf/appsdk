import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import net from 'node:net';
import path from 'node:path';
import {spawn} from 'node:child_process';
import {fileURLToPath} from 'node:url';

const script=process.env.COLLAB_TEST_DESKTOP_WRAPPER??fileURLToPath(new URL('../collab-desktop-mcp.mjs',import.meta.url));
const binary=process.env.COLLAB_TEST_BINARY;
const frame=v=>{const b=Buffer.from(JSON.stringify(v)),h=Buffer.alloc(4);h.writeUInt32LE(b.length);return Buffer.concat([h,b]);};
function decode(s,fn){let b=Buffer.alloc(0);s.on('data',c=>{b=Buffer.concat([b,c]);while(b.length>=4&&b.length>=4+b.readUInt32LE(0)){const n=b.readUInt32LE(0);fn(JSON.parse(b.subarray(4,4+n)));b=b.subarray(4+n);}});}
function capabilityPath(state,cwd,session,thread){let p=path.join(state,'runtime','desktop','capabilities','v1');for(const [label,value] of [['cwd',cwd],['session',session],['thread',thread]])p=path.join(p,label,...Buffer.from(value).toString('hex').match(/.{1,120}/g));return path.join(p,'current.json');}

async function fixture(t){
  const root=await fs.mkdtemp('/tmp/cdg-'),state=path.join(root,'state'),calls=[],streams=new Set();
  await fs.mkdir(state,{mode:0o700});
  const server=net.createServer(s=>{streams.add(s);s.on('close',()=>streams.delete(s));decode(s,r=>{
    calls.push(r);
    if(r.method==='tools/list'){s.write(frame({id:r.id,result:{tools:['read_thread','send_message_to_thread'].map(name=>({name,namespace:'codex_app'}))}}));return;}
    const p=r.params;
    if(p.tool==='read_thread')s.write(frame({id:r.id,result:{success:p.arguments.threadId!=='unknown',contentItems:[{type:'inputText',text:JSON.stringify({schemaVersion:1,thread:{id:p.arguments.threadId,cwd:root,status:{type:'idle'}},turns:[{id:'observed-turn',status:'completed'}]})}]}}));
    else s.write(frame({id:r.id,result:{success:true,contentItems:[]}}));
  });});
  await new Promise(r=>server.listen(path.join(root,'host.sock'),r));
  const pipe=path.join(root,'host.sock');
  const hostEnv={PATH:process.env.PATH,COLLAB_STATE_DIR:state,CODEX_APP_TOOLS_PIPE_PATH:pipe,CODEX_MCP_NODE_PATH:process.execPath,COLLAB_DESKTOP_BRIDGE_SCRIPT:script};
  const admit=(session,thread,cwd=root)=>new Promise((resolve,reject)=>{
    const p=spawn(process.execPath,[script,'--admit',JSON.stringify({session_id:session,thread_id:thread,cwd})],{env:{...hostEnv,CODEX_INTERNAL_ORIGINATOR_OVERRIDE:'Codex'},stdio:['ignore','pipe','pipe']});
    let out='',err='';p.stdout.on('data',b=>out+=b);p.stderr.on('data',b=>err+=b);
    const timer=setTimeout(()=>{p.kill('SIGTERM');reject(Error('admit timeout'));},8000);
    p.on('exit',code=>{clearTimeout(timer);if(code!==0)return reject(Error(err||out||'admit failed'));try{resolve(JSON.parse(out));}catch{reject(Error('no admission result: '+out+err));}});
  });
  const reads=match=>calls.filter(c=>c.params?.tool==='read_thread'&&c.params?.threadId===match).length;
  t.after(async()=>{for(const s of streams)s.destroy();await new Promise(r=>server.close(r));await fs.rm(root,{recursive:true,force:true});});
  return {root,state,calls,pipe,hostEnv,admit,reads};
}

async function startDaemon(t,f,root){
  const home=path.join(root,'home');
  await fs.mkdir(home,{recursive:true});
  await fs.mkdir(path.join(root,'.agent-collab'),{recursive:true});
  const env={PATH:process.env.PATH,HOME:home,CODEX_HOME:home,COLLAB_STATE_DIR:f.state,CODEX_INTERNAL_ORIGINATOR_OVERRIDE:'Codex TUI'};
  const daemon=spawn(binary,['serve'],{cwd:root,env,stdio:'ignore'});
  t.after(async()=>{if(daemon.exitCode===null&&daemon.signalCode===null){const closed=new Promise(r=>daemon.once('close',r));daemon.kill('SIGTERM');await closed;}});
  const deadline=Date.now()+10000;
  while(!(await fs.stat(path.join(f.state,'server.sock')).catch(()=>null))){
    assert.ok(daemon.exitCode===null&&Date.now()<deadline,'isolated daemon failed to start');
    await new Promise(r=>setTimeout(r,25));
  }
  return home;
}

/** Register the fixture thread through the documented launcher shape so the
 *  enrollment and identity binding exist before the managed request. */
function setupRegistration(t,root,home,f,session,thread){
  const env={...f.hostEnv,HOME:home,CODEX_HOME:home,COLLAB_BIN:binary,COLLAB_MCP_BIN:path.join(path.dirname(binary),'collab-mcp'),
    CODEX_INTERNAL_ORIGINATOR_OVERRIDE:'Codex Desktop',CODEX_SESSION_ID:session,CODEX_THREAD_ID:thread};
  return new Promise((resolve,reject)=>{
    const child=spawn(binary,['context'],{cwd:root,env,stdio:['ignore','pipe','pipe']});
    let out='',err='';child.stdout.on('data',b=>out+=b);child.stderr.on('data',b=>err+=b);
    child.on('close',code=>{if(code!==0)return reject(Error(out+err));try{resolve(JSON.parse(out));}catch{reject(Error('setup registration without JSON: '+out+err));}});
  });
}

/** One managed Desktop MCP lifecycle: the host passes only the pipe, the real
 *  originator and the current `_meta` thread. No session environment and no
 *  caller-supplied facts are present. */
async function managed(t,root,f,{origin='Codex',tool='collab_context',args={},thread='real-thread',cwd=root}={}){
  const env={...f.hostEnv,HOME:path.join(root,'home'),CODEX_HOME:path.join(root,'home'),COLLAB_BIN:binary,
    COLLAB_MCP_BIN:path.join(path.dirname(binary),'collab-mcp'),CODEX_INTERNAL_ORIGINATOR_OVERRIDE:origin};
  const child=spawn(process.execPath,[script],{cwd,env,stdio:['pipe','pipe','pipe']});
  const {createInterface}=await import('node:readline');
  const lines=createInterface({input:child.stdout})[Symbol.asyncIterator]();
  let stderr='';child.stderr.on('data',b=>stderr+=b);
  const closed=new Promise(resolve=>child.once('close',resolve));
  const timer=setTimeout(()=>child.kill('SIGTERM'),20000);
  async function rpc(id,method,params){
    child.stdin.write(JSON.stringify({jsonrpc:'2.0',id,method,params})+'\n');
    for(;;){const line=await lines.next();assert.ok(!line.done,`MCP closed before responding: ${stderr}`);const value=JSON.parse(line.value);if(value.id===id)return value;}
  }
  try{
    await rpc('init','initialize',{protocolVersion:'2024-11-05',capabilities:{},clientInfo:{name:'caller-gate',version:'1'}});
    const before=f.calls.length;
    const response=await rpc('call','tools/call',{name:tool,arguments:args,_meta:{'openai/threadId':thread,'x-codex-turn-metadata':{thread_id:thread,turn_id:'request-turn'}}});
    return Object.assign({},response,{starts:f.calls.length,reads:f.calls.slice(before).filter(c=>c.params?.tool==='read_thread').length,stderr});
  }finally{child.stdin.end();await closed;clearTimeout(timer);}
}

const text=response=>response.result?.content?.[0]?.text??'';
const isError=response=>response.result?.isError===true;

test('managed originator registers its enrolled caller through the verified host thread',async t=>{
  if(!binary){t.skip('Rust CLI consumer runs with COLLAB_TEST_BINARY');return;}
  const f=await fixture(t),root=await fs.realpath(f.root),home=await startDaemon(t,f,root);
  const setup=await setupRegistration(t,root,home,f,'enrolled-session','real-thread');
  const worker=setup.result.snapshot.identity.worker_id;
  const response=await managed(t,root,f,{args:{project_scope:root,app_scope_id:'appserver-cli'}});
  assert.equal(isError(response),false,`managed registration must succeed: ${JSON.stringify(response)}`);
  const snapshot=JSON.parse(text(response)).result.snapshot;
  assert.equal(snapshot.binding.session_id,'enrolled-session',JSON.stringify(snapshot));
  assert.equal(snapshot.identity.worker_id,worker,JSON.stringify(snapshot));
  assert.ok(f.calls.some(c=>c.params?.tool==='read_thread'&&c.params?.threadId==='real-thread'),'the host must verify the request thread');
});

test('managed query resolves the same enrolled caller without reading the current declaration',async t=>{
  if(!binary){t.skip('Rust CLI consumer runs with COLLAB_TEST_BINARY');return;}
  const f=await fixture(t),root=await fs.realpath(f.root),home=await startDaemon(t,f,root);
  const setup=await setupRegistration(t,root,home,f,'enrolled-session','real-thread');
  const worker=setup.result.snapshot.identity.worker_id;
  const current=capabilityPath(f.state,root,'enrolled-session','real-thread');
  const corrupt=Buffer.from('not a capability declaration\n');
  await fs.writeFile(current,corrupt);
  const identityMap=path.join(f.state,'identity-map.json'),before=await fs.readFile(identityMap);
  const response=await managed(t,root,f,{args:{query:true,project_scope:root,app_scope_id:'appserver-cli'}});
  assert.equal(isError(response),false,`query must use the trusted path without the declaration: ${JSON.stringify(response)}`);
  const snapshot=JSON.parse(text(response)).result.snapshot;
  assert.equal(snapshot.identity.worker_id,worker,JSON.stringify(snapshot));
  assert.equal(snapshot.binding.session_id,'enrolled-session',JSON.stringify(snapshot));
  assert.ok(f.calls.some(c=>c.params?.tool==='read_thread'&&c.params?.threadId==='real-thread'),'query must verify the request thread with the host');
  assert.deepEqual(await fs.readFile(identityMap),before,'query must not mutate the identity map');
  assert.deepEqual(await fs.readFile(current),corrupt,'query must not admit or rewrite the current declaration');
});

test('managed request fails closed when no enrollment matches the verified thread',async t=>{
  if(!binary){t.skip('Rust CLI consumer runs with COLLAB_TEST_BINARY');return;}
  const f=await fixture(t),root=await fs.realpath(f.root);
  await f.admit('other-session','other-thread',root);
  const resources=await fs.readdir(f.state);
  const response=await managed(t,root,f,{args:{project_scope:root,app_scope_id:'appserver-cli'}});
  assert.equal(isError(response),true,`zero candidates must fail closed: ${JSON.stringify(response)}`);
  assert.match(text(response),/DESKTOP_BRIDGE_SESSION_REQUIRED/,text(response));
  assert.deepEqual(await fs.readdir(f.state),resources,'a denied request must not create state');
});

test('managed request fails closed when two distinct sessions match the verified thread',async t=>{
  if(!binary){t.skip('Rust CLI consumer runs with COLLAB_TEST_BINARY');return;}
  const f=await fixture(t),root=await fs.realpath(f.root);
  await f.admit('first-session','real-thread',root);
  await f.admit('second-session','real-thread',root);
  const records=await fs.readdir(path.join(f.state,'runtime','desktop','enrollments'));
  assert.equal(records.length,2,'the fixture must hold two distinct sessions for one thread');
  const response=await managed(t,root,f,{args:{project_scope:root,app_scope_id:'appserver-cli'}});
  assert.equal(isError(response),true,`multiple candidates must fail closed: ${JSON.stringify(response)}`);
  assert.match(text(response),/DESKTOP_BRIDGE_SESSION_AMBIGUOUS/,text(response));
});

test('standalone launcher originators stay outside the managed caller path',async t=>{
  if(!binary){t.skip('Rust CLI consumer runs with COLLAB_TEST_BINARY');return;}
  const f=await fixture(t),root=await fs.realpath(f.root);
  await f.admit('enrolled-session','real-thread',root);
  const admitted=f.reads('real-thread');
  for(const origin of ['Codex CLI','Codex TUI']){
    const response=await managed(t,root,f,{origin,args:{query:true,project_scope:root,app_scope_id:'appserver-cli'}});
    assert.equal(isError(response),true,`${origin} must not adopt Desktop caller facts: ${JSON.stringify(response)}`);
    assert.doesNotMatch(text(response),/DESKTOP_BRIDGE_SESSION_(REQUIRED|AMBIGUOUS)/,text(response));
    assert.equal(f.reads('real-thread'),admitted,`${origin} must not verify the managed host thread`);
  }
});
