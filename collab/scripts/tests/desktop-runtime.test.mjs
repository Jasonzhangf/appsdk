import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import net from 'node:net';
import path from 'node:path';
import {spawn} from 'node:child_process';
import {fileURLToPath} from 'node:url';

const script=process.env.COLLAB_TEST_DESKTOP_WRAPPER??fileURLToPath(new URL('../collab-desktop-mcp.mjs',import.meta.url));
const frame=v=>{const b=Buffer.from(JSON.stringify(v)),h=Buffer.alloc(4);h.writeUInt32LE(b.length);return Buffer.concat([h,b]);};
function decode(s,fn){let b=Buffer.alloc(0);s.on('data',c=>{b=Buffer.concat([b,c]);while(b.length>=4&&b.length>=4+b.readUInt32LE(0)){const n=b.readUInt32LE(0);fn(JSON.parse(b.subarray(4,4+n)));b=b.subarray(4+n);}});}
async function fixture(t){
  const root=await fs.mkdtemp('/tmp/cdr-'),state=path.join(root,'state'),calls=[],streams=new Set();
  await fs.mkdir(state,{mode:0o700});
  const server=net.createServer(s=>{streams.add(s);s.on('close',()=>streams.delete(s));decode(s,r=>{
    calls.push(r);
    if(r.method==='tools/list'){s.write(frame({id:r.id,result:{tools:['read_thread','send_message_to_thread'].map(name=>({name,namespace:'codex_app'}))}}));return;}
    const p=r.params;
    if(p.tool==='read_thread')s.write(frame({id:r.id,result:{success:p.arguments.threadId!=='unknown',contentItems:[{type:'inputText',text:JSON.stringify({schemaVersion:1,thread:{id:p.arguments.threadId,cwd:root,status:{type:'idle'}},turns:[{id:'observed-turn',status:'completed'}]})}]}}));
    else if(p.arguments.prompt==='disconnect')s.destroy();
    else s.write(frame({id:r.id,result:{success:p.arguments.prompt!=='reject',contentItems:[]}}));
  });});
  await new Promise(r=>server.listen(path.join(root,'host.sock'),r));
  const env={...process.env,COLLAB_STATE_DIR:state,CODEX_APP_TOOLS_PIPE_PATH:path.join(root,'host.sock'),CODEX_MCP_NODE_PATH:process.execPath,CODEX_INTERNAL_ORIGINATOR_OVERRIDE:'Codex Desktop'};
  async function admit(session,thread,cwd=root){return await new Promise((resolve,reject)=>{const p=spawn(process.execPath,[script,'--admit',JSON.stringify({session_id:session,thread_id:thread,cwd})],{env,stdio:['ignore','pipe','pipe']});let out='',err='';p.stdout.on('data',b=>out+=b);p.stderr.on('data',b=>err+=b);const timer=setTimeout(()=>{p.kill('SIGTERM');reject(Error('admit timeout'));},8000);p.on('exit',code=>{clearTimeout(timer);if(code!==0)return reject(Error(err||out||'admit failed'));try{resolve(JSON.parse(out));}catch{reject(Error('no runtime admission result: '+out+err));}});});}
  async function request(candidate,request){return await new Promise((resolve,reject)=>{const s=net.createConnection(candidate.endpoint.slice(7));const timer=setTimeout(()=>{s.destroy();reject(Error('request timeout'));},7000);s.on('error',e=>{clearTimeout(timer);reject(e);});decode(s,r=>{clearTimeout(timer);s.end();resolve(r);});s.on('connect',()=>s.write(frame(request)));});}
  t.after(async()=>{for(const s of streams)s.destroy();await new Promise(r=>server.close(r));await fs.rm(root,{recursive:true,force:true});});
  return {root,state,calls,env,admit,request,streams};
}

test('fresh sessions share one runtime service without MCP context or wrapper lifetime',async t=>{
  const f=await fixture(t);
  const [a,b]=await Promise.all([f.admit('session-a','thread-a'),f.admit('session-b','thread-b')]);
  assert.equal(a.endpoint,b.endpoint);
  assert.equal((await f.request(a,{id:1,op:'send',session_id:'session-a',thread_id:'thread-a',text:'hello'})).ok,true);
  assert.equal((await f.request(b,{id:2,op:'send',session_id:'session-b',thread_id:'thread-b',text:'world'})).ok,true);
  assert.ok(f.calls.filter(c=>c.method==='tools/call').every(c=>c.params.turnId.startsWith('collab-service-correlate-')));
  assert.equal(new Set(f.calls.filter(c=>c.method==='tools/call').map(c=>c.params.threadId)).size,2);
});

test('runtime admission rejects wrong project and unknown target without harming enrolled session',async t=>{
  const f=await fixture(t),a=await f.admit('a','known');
  await assert.rejects(f.admit('x','unknown'),/NATIVE|PUBLICATION|ADMISSION/);
  await assert.rejects(f.admit('b','other',f.state),/SCOPE_MISMATCH/);
  const invalid=await f.request(a,{id:1,op:'send',session_id:'b',thread_id:'other',text:'bad'});
  assert.equal(invalid.ok,false);
  assert.equal((await f.request(a,{id:2,op:'send',session_id:'a',thread_id:'known',text:'reject'})).ok,false);
  assert.equal((await f.request(a,{id:3,op:'send',session_id:'a',thread_id:'known',text:'valid'})).ok,true);
});

test('public CLI registers fresh sessions without MCP and ignores retired local journal',async t=>{
  const binary=process.env.COLLAB_TEST_BINARY;
  if(!binary){t.skip('Rust CLI consumer runs with COLLAB_TEST_BINARY');return;}
  const f=await fixture(t);await fs.mkdir(path.join(f.root,'.agent-collab'));await fs.mkdir(path.join(f.root,'home'));
  const legacy=path.join(f.root,'.agent-collab','server','journal.jsonl');
  await fs.mkdir(path.dirname(legacy));await fs.writeFile(legacy,'incompatible-retired-version\n');
  const env={...f.env,HOME:path.join(f.root,'home'),CODEX_HOME:path.join(f.root,'home'),COLLAB_DESKTOP_BRIDGE_SCRIPT:script};
  const daemonEnv={...env,CODEX_INTERNAL_ORIGINATOR_OVERRIDE:'Codex TUI'};delete daemonEnv.CODEX_APP_TOOLS_PIPE_PATH;
  let daemon,daemonLog='';
  function startDaemon(){daemon=spawn(binary,['serve'],{cwd:f.root,env:daemonEnv,stdio:['ignore','ignore','pipe']});daemon.stderr.on('data',b=>daemonLog+=b);}
  startDaemon();
  t.after(async()=>{if(daemon.exitCode===null){daemon.kill('SIGTERM');await new Promise(r=>daemon.once('exit',r));}});
  const end=Date.now()+10000;while(Date.now()<end){try{await fs.stat(path.join(f.state,'server.sock'));break;}catch{}if(daemon.exitCode!==null)throw Error('fixture daemon failed: '+daemonLog);await new Promise(r=>setTimeout(r,30));}
  async function cli(session,thread,withoutNode=false){return await new Promise((resolve,reject)=>{const currentEnv={...env,CODEX_SESSION_ID:session,CODEX_THREAD_ID:thread};if(withoutNode)delete currentEnv.CODEX_MCP_NODE_PATH;const child=spawn(binary,['context'],{cwd:f.root,env:currentEnv,stdio:['ignore','pipe','pipe']});let out='',err='';child.stdout.on('data',b=>out+=b);child.stderr.on('data',b=>err+=b);child.on('exit',code=>code===0?resolve(out):reject(Error(out+err)));});}
  const a=await cli('fresh-a','thread-a'),b=await cli('fresh-b','thread-b',true);
  assert.match(a,/committed|completed/);assert.match(b,/committed|completed/);
  assert.equal(await fs.readFile(legacy,'utf8'),'incompatible-retired-version\n');
  const x=await f.admit('fresh-a','thread-a'),y=await f.admit('fresh-b','thread-b');assert.equal(x.endpoint,y.endpoint);
  assert.equal((await f.request(y,{id:'after-cli',op:'send',session_id:'fresh-b',thread_id:'thread-b',text:'from CLI registration'})).ok,true);
  async function stopDaemon(){const done=new Promise(r=>daemon.once('exit',r));daemon.kill('SIGTERM');await done;}
  await stopDaemon();startDaemon();
  const restartEnd=Date.now()+5000;while(Date.now()<restartEnd){try{await fs.stat(path.join(f.state,'server.sock'));break;}catch{}await new Promise(r=>setTimeout(r,30));}
  const restored=await cli('fresh-a','thread-a');assert.match(restored,/completed/);
  await stopDaemon();
  async function journals(dir){const result=[];for(const e of await fs.readdir(dir,{withFileTypes:true})){const p=path.join(dir,e.name);if(e.isDirectory())result.push(...await journals(p));else if(e.name==='journal.jsonl')result.push(p);}return result;}
  const central=await journals(path.join(f.state,'projects'));assert.ok(central.length>0);
  for(const journal of central)await fs.writeFile(journal,'invalid-current-central-journal\n');
  startDaemon();
  const corruptEnd=Date.now()+5000;while(Date.now()<corruptEnd){try{await fs.stat(path.join(f.state,'server.sock'));break;}catch{}await new Promise(r=>setTimeout(r,30));}
  await assert.rejects(cli('fresh-a','thread-a'),/journal|PROJECT_RUNTIME_UNAVAILABLE|degraded/i);
  for(const journal of central)assert.equal(await fs.readFile(journal,'utf8'),'invalid-current-central-journal\n');
  assert.equal(await fs.readFile(legacy,'utf8'),'incompatible-retired-version\n');
});
