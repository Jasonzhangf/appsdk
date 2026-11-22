import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import net from 'node:net';
import path from 'node:path';
import {spawn} from 'node:child_process';
import {fileURLToPath} from 'node:url';

const script=process.env.COLLAB_TEST_DESKTOP_WRAPPER??fileURLToPath(new URL('../collab-desktop-mcp.mjs',import.meta.url));
const frame=v=>{const b=Buffer.from(JSON.stringify(v)),h=Buffer.alloc(4);h.writeUInt32LE(b.length);return Buffer.concat([h,b]);};
function capabilityPath(state,cwd,session,thread){let p=path.join(state,'runtime','desktop','capabilities','v1');for(const [label,value] of [['cwd',cwd],['session',session],['thread',thread]])p=path.join(p,label,...Buffer.from(value).toString('hex').match(/.{1,120}/g));return path.join(p,'current.json');}
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
  async function stopService(candidate){const ping=await request(candidate,{id:'owned-service',op:'ping'});assert.ok(Number.isInteger(ping.pid)&&ping.pid>1);process.kill(ping.pid,'SIGTERM');const deadline=Date.now()+5000;while(Date.now()<deadline){try{await fs.stat(candidate.endpoint.slice('unix://'.length));}catch(e){if(e.code==='ENOENT')return;throw e;}await new Promise(r=>setTimeout(r,20));}throw Error('owned runtime service did not stop');}
  t.after(async()=>{for(const s of streams)s.destroy();await new Promise(r=>server.close(r));await fs.rm(root,{recursive:true,force:true});});
  return {root,state,calls,env,admit,request,stopService,streams};
}

async function removeEnrollment(state,session,thread){
  async function walk(dir){
    for(const e of await fs.readdir(dir,{withFileTypes:true})){
      const p=path.join(dir,e.name);
      if(e.isDirectory()){await walk(p);continue;}
      if(!e.name.endsWith('.json'))continue;
      const value=JSON.parse(await fs.readFile(p,'utf8'));
      if(value.binding?.session===session&&value.binding?.thread===thread)await fs.rm(p,{force:true});
    }
  }
  const root=path.join(state,'runtime','desktop','enrollments');
  try{await walk(root);}catch(e){if(e.code!=='ENOENT')throw e;}
}

async function mcpContext(command,args,{cwd,env,timeoutMs=15000}){
  const child=spawn(command,args,{cwd,env,stdio:['pipe','pipe','pipe']});
  let stdout='',stderr='';
  child.stdout.on('data',chunk=>stdout+=chunk);
  child.stderr.on('data',chunk=>stderr+=chunk);
  child.stdin.end([
    JSON.stringify({jsonrpc:'2.0',id:'initialize',method:'initialize',params:{protocolVersion:'2024-11-05',capabilities:{},clientInfo:{name:'desktop-gate',version:'1'}}}),
    JSON.stringify({jsonrpc:'2.0',id:'context',method:'tools/call',params:{name:'collab_context',arguments:{},_meta:{'x-codex-turn-metadata':{thread_id:'real-thread',turn_id:'real-origin-turn'}}}}),
  ].join('\n')+'\n');
  const timer=setTimeout(()=>child.kill('SIGTERM'),timeoutMs);
  const code=await new Promise((resolve,reject)=>{child.on('error',reject);child.on('exit',resolve);});
  clearTimeout(timer);
  if(code!==0)throw Error(`MCP exited ${code}: ${stderr}`);
  const response=stdout.trim().split('\n').map(line=>JSON.parse(line)).find(value=>value.id==='context');
  if(!response)throw Error(`MCP context response missing: ${stdout}${stderr}`);
  return response;
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

test('public CLI restores current state and rejects corrupt current state without reading local legacy bytes',async t=>{
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
  const canonical=await fs.realpath(f.root),routes=JSON.parse(await fs.readFile(path.join(f.state,'routes.json'),'utf8'));
  const matched=routes.filter(route=>route.project_scope===canonical&&route.app_scope_id==='appserver-cli');
  assert.equal(matched.length,1);
  const central=path.join(matched[0].storage_root,'.agent-collab','server','state.json');
  const relative=path.relative(path.join(await fs.realpath(f.state),'projects'),central);
  assert.ok(relative!==''&&!relative.startsWith('..')&&!path.isAbsolute(relative));
  await fs.writeFile(central,'invalid-current-state\n');
  startDaemon();
  const code=await new Promise((resolve,reject)=>{const timer=setTimeout(()=>reject(Error('corrupt current state did not fail startup')),5000);daemon.once('exit',code=>{clearTimeout(timer);resolve(code);});});
  assert.equal(typeof code,'number');assert.notEqual(code,0);
  assert.match(daemonLog,/CURRENT_STATE_DECODE_FAILED/);
  await assert.rejects(fs.stat(path.join(f.state,'server.sock')),{code:'ENOENT'});
  assert.equal(await fs.readFile(central,'utf8'),'invalid-current-state\n');
  assert.equal(await fs.readFile(legacy,'utf8'),'incompatible-retired-version\n');
});

test('direct MCP cannot supply a Desktop session while the formal wrapper admits the same isolated host',async t=>{
  const binary=process.env.COLLAB_TEST_BINARY;
  if(!binary){t.skip('Rust CLI consumer runs with COLLAB_TEST_BINARY');return;}
  const f=await fixture(t),home=path.join(f.root,'home');
  await fs.mkdir(home);await fs.mkdir(path.join(f.root,'.agent-collab'));
  const daemonEnv={...f.env,HOME:home,CODEX_HOME:home,CODEX_INTERNAL_ORIGINATOR_OVERRIDE:'Codex TUI'};
  delete daemonEnv.CODEX_APP_TOOLS_PIPE_PATH;delete daemonEnv.CODEX_MCP_NODE_PATH;
  let daemon,daemonLog='';
  function startDaemon(){daemon=spawn(binary,['serve'],{cwd:f.root,env:daemonEnv,stdio:['ignore','ignore','pipe']});daemon.stderr.on('data',chunk=>daemonLog+=chunk);}
  startDaemon();
  t.after(async()=>{if(daemon.exitCode===null){daemon.kill('SIGTERM');await new Promise(resolve=>daemon.once('exit',resolve));}});
  const deadline=Date.now()+10000;
  while(Date.now()<deadline){try{await fs.stat(path.join(f.state,'server.sock'));break;}catch{}if(daemon.exitCode!==null)throw Error('fixture daemon failed: '+daemonLog);await new Promise(resolve=>setTimeout(resolve,30));}
  const mcp=process.env.COLLAB_TEST_MCP_BINARY??path.join(path.dirname(binary),'collab-mcp');
  const baseEnv={...f.env,HOME:home,CODEX_HOME:home,COLLAB_BIN:binary};
  delete baseEnv.CODEX_SESSION_ID;delete baseEnv.CODEX_THREAD_ID;
  const direct=await mcpContext(mcp,[],{cwd:f.root,env:baseEnv});
  assert.equal(direct.result.isError,true);
  assert.match(direct.result.content[0].text,/DESKTOP_BRIDGE_SESSION_REQUIRED/);
  const enrolled=f.admit('agent-session','real-thread');await enrolled;
  const wrapperEnv={...baseEnv,COLLAB_MCP_BIN:mcp};
  const formal=await mcpContext(process.execPath,[script],{cwd:f.root,env:wrapperEnv});
  assert.equal(formal.result.isError,false);
  assert.match(formal.result.content[0].text,/committed|completed/);
  assert.ok(f.calls.some(call=>call.params?.tool==='read_thread'&&call.params?.threadId==='real-thread'),'admission and wrapper verify the same isolated executor thread');
});

test('managed wrapper verifies the request thread and queries its enrolled project from another cwd',async t=>{
  const binary=process.env.COLLAB_TEST_BINARY;
  if(!binary){t.skip('Rust CLI consumer runs with COLLAB_TEST_BINARY');return;}
  const f=await fixture(t),project=await fs.realpath(f.root),home=path.join(project,'home');
  await fs.mkdir(home);await fs.mkdir(path.join(project,'.agent-collab'));
  const session='enrolled-session',thread='real-thread';
  const env={PATH:process.env.PATH,HOME:home,CODEX_HOME:home,COLLAB_STATE_DIR:f.state,
    CODEX_APP_TOOLS_PIPE_PATH:f.env.CODEX_APP_TOOLS_PIPE_PATH,CODEX_MCP_NODE_PATH:process.execPath,
    COLLAB_DESKTOP_BRIDGE_SCRIPT:script,COLLAB_BIN:binary,
    COLLAB_MCP_BIN:path.join(path.dirname(binary),'collab-mcp'),CODEX_INTERNAL_ORIGINATOR_OVERRIDE:'Codex',CODEX_SESSION_ID:'foreign-session'};
  const daemon=spawn(binary,['serve'],{cwd:project,env:{PATH:env.PATH,HOME:home,COLLAB_STATE_DIR:f.state},stdio:'ignore'});
  try{
    const deadline=Date.now()+10000;
    while(!(await fs.stat(path.join(f.state,'server.sock')).catch(()=>null))){
      assert.ok(daemon.exitCode===null&&Date.now()<deadline,'isolated daemon failed to start');
      await new Promise(resolve=>setTimeout(resolve,25));
    }
    const setup=spawn(binary,['context'],{cwd:project,env:{...env,CODEX_INTERNAL_ORIGINATOR_OVERRIDE:'Codex Desktop',CODEX_SESSION_ID:session,CODEX_THREAD_ID:thread},stdio:['ignore','pipe','pipe']});
    let out='',err='';setup.stdout.on('data',b=>out+=b);setup.stderr.on('data',b=>err+=b);
    assert.equal(await new Promise(r=>setup.once('close',r)),0,out+err);
    const snapshot=JSON.parse(out).result.snapshot;
    assert.equal(snapshot.binding.session_id,session);
    assert.ok(snapshot.identity.worker_id);
    const records=await fs.readdir(path.join(f.state,'runtime','desktop','enrollments'));
    assert.equal(records.length,1);
    const enrollment=JSON.parse(await fs.readFile(path.join(f.state,'runtime','desktop','enrollments',records[0])));
    assert.deepEqual(enrollment.binding,{session,thread});
    assert.equal(enrollment.cwd,project);assert.equal(enrollment.host_pipe,env.CODEX_APP_TOOLS_PIPE_PATH);
    const identityMap=path.join(f.state,'identity-map.json'),before=await fs.readFile(identityMap);
    const {createInterface}=await import('node:readline');
    async function query(from,label,tool='collab_context',args={query:true}){
      const wrapper=spawn(process.execPath,[script],{cwd:from,env,stdio:['pipe','pipe','pipe']});
      const closed=new Promise(resolve=>wrapper.once('close',resolve));
      const timer=setTimeout(()=>wrapper.kill('SIGTERM'),15000);
      const lines=createInterface({input:wrapper.stdout})[Symbol.asyncIterator]();
      async function rpc(id,method,params){
        wrapper.stdin.write(JSON.stringify({jsonrpc:'2.0',id,method,params})+'\n');
        for(;;){const line=await lines.next();assert.ok(!line.done,`${label}: wrapper closed before responding`);
          const response=JSON.parse(line.value);if(response.id===id)return response;}
      }
      try{
        await rpc('init','initialize',{protocolVersion:'2024-11-05',capabilities:{},clientInfo:{name:'enrolled-query',version:'1'}});
        const start=f.calls.length;
        const response=await rpc('query','tools/call',{name:tool,arguments:tool==='collab_context'?{query:true,project_scope:project,app_scope_id:'appserver-cli'}:args,_meta:{'openai/threadId':thread,'x-codex-turn-metadata':{thread_id:thread,turn_id:'request-turn'}}});
        const reads=f.calls.slice(start).filter(c=>c.params?.tool==='read_thread'&&c.params.threadId===thread&&c.params.arguments.threadId===thread);
        return {response,reads:reads.length};
      }finally{wrapper.stdin.end();await closed;clearTimeout(timer);}
    }
    // The request names its project scope; the process cwd must not decide which
    // enrollment answers it, so both shapes are observed in one run.
    const elsewhere=await query(home,'cwd outside the project');
    const inside=await query(project,'cwd at the project');
    assert.deepEqual(await fs.readFile(identityMap),before,'a query must not mutate the identity map');
    assert.ok(elsewhere.reads>0,`query from another cwd made ${elsewhere.reads} exact native read_thread calls: ${JSON.stringify(elsewhere.response)}`);
    assert.equal(elsewhere.response.result.isError,false,JSON.stringify(elsewhere.response));
    const result=JSON.parse(elsewhere.response.result.content[0].text).result.snapshot;
    assert.equal(result.identity.worker_id,snapshot.identity.worker_id);
    assert.equal(result.binding.session_id,session);
    assert.ok(inside.reads>0,`query at the project made ${inside.reads} exact native read_thread calls: ${JSON.stringify(inside.response)}`);
    assert.equal(inside.response.result.isError,false,JSON.stringify(inside.response));
    assert.equal(JSON.parse(inside.response.result.content[0].text).result.snapshot.binding.session_id,session);
    // Board and task reads carry no scope argument, and a real Codex Desktop
    // wrapper runs from `/`. The native read_thread answer names the project,
    // so every managed tool must serve the enrolled project from any cwd, and
    // each read must verify the request thread natively.
    // A real board entry so collab_board_describe can return its projection.
    const publish=spawn(binary,['board','publish','r2-describe-task','--title','R2 describe','--description','real entry','--delivery-condition','delivered','--test-condition','verified'],{cwd:project,env:{...env,CODEX_INTERNAL_ORIGINATOR_OVERRIDE:'Codex Desktop',CODEX_SESSION_ID:session,CODEX_THREAD_ID:thread},stdio:['ignore','pipe','pipe']});
    let publishOut='',publishErr='';publish.stdout.on('data',b=>publishOut+=b);publish.stderr.on('data',b=>publishErr+=b);
    assert.equal(await new Promise(r=>publish.once('close',r)),0,publishOut+publishErr);
    // describe writes the description back, so it advances the revision each
    // call; track the current one instead of asserting a fixed value.
    let describeRevision=1;
    const describeArgs=()=>({id:'r2-describe-task',expected_revision:describeRevision,title:'R2 describe',description:'real entry',delivery_condition:'delivered',test_condition:'verified'});
    const boardArgs={collab_task_status:{},collab_board_show:{}};
    for(const tool of ['collab_task_status','collab_board_show']){
      const read=await query(project,`${tool} at the project`,tool,boardArgs[tool]);
      assert.equal(read.response.result.isError,false,`${tool}: ${JSON.stringify(read.response)}`);
      assert.ok(read.reads>0,`${tool} made ${read.reads} exact native read_thread calls`);
      const text=read.response.result.content[0].text;
      assert.doesNotMatch(text,/DESKTOP_BRIDGE_SESSION_(REQUIRED|AMBIGUOUS)/,`${tool}: ${text}`);
      assert.doesNotMatch(text,/DESKTOP_BRIDGE_SCOPE_MISMATCH/,`${tool}: ${text}`);
      const root=await query('/',`${tool} from the wrapper root`,tool,boardArgs[tool]);
      assert.ok(root.reads>0,`${tool} from / verified the thread natively ${root.reads} times`);
      assert.equal(root.response.result.isError,false,`${tool} from / must serve the enrolled project: ${JSON.stringify(root.response)}`);
      assert.ok(root.reads>0,`${tool} from / made ${root.reads} exact native read_thread calls`);
      const rootText=root.response.result.content[0].text;
      assert.doesNotMatch(rootText,/DESKTOP_BRIDGE_SESSION_(REQUIRED|AMBIGUOUS)/,`${tool} from /: ${rootText}`);
      assert.doesNotMatch(rootText,/DESKTOP_BRIDGE_SCOPE_MISMATCH/,`${tool} from /: ${rootText}`);
    }
    // collab_board_describe must work from `/` too: the wrapper cwd never
    // decides its project scope.
    const describeAtProject=await query(project,'collab_board_describe at the project','collab_board_describe',describeArgs());
    assert.equal(describeAtProject.response.result.isError,false,`collab_board_describe: ${JSON.stringify(describeAtProject.response)}`);
    assert.ok(describeAtProject.reads>0,'collab_board_describe made native read_thread calls');
    describeRevision+=1;
    const describeAtRoot=await query('/','collab_board_describe from the wrapper root','collab_board_describe',describeArgs());
    assert.equal(describeAtRoot.response.result.isError,false,`collab_board_describe from / must serve the enrolled project: ${JSON.stringify(describeAtRoot.response)}`);
    assert.ok(describeAtRoot.reads>0,'collab_board_describe from / made native read_thread calls');
    for(const text of [describeAtProject.response.result.content[0].text,describeAtRoot.response.result.content[0].text])
      assert.doesNotMatch(text,/DESKTOP_BRIDGE_(SESSION_(REQUIRED|AMBIGUOUS)|SCOPE_MISMATCH)/,`collab_board_describe: ${text}`);
    // collab_recv resolves the same caller on its ordinary managed path: it
    // takes no scope, must answer with its receive projection rather than a
    // bridge rejection, and must still verify the thread natively.
    const recv=await query('/','collab_recv from the wrapper root','collab_recv',{timeout:1,receive_id:'receive-enrolled'});
    assert.equal(recv.response.result.isError,false,`collab_recv: ${JSON.stringify(recv.response)}`);
    assert.ok(recv.reads>0,`collab_recv made ${recv.reads} exact native read_thread calls`);
    assert.doesNotMatch(recv.response.result.content[0].text,/DESKTOP_BRIDGE_/,recv.response.result.content[0].text);
    assert.deepEqual(await fs.readFile(identityMap),before,'managed reads must not mutate the identity map');
  }finally{
    if(daemon.exitCode===null&&daemon.signalCode===null){const stopped=new Promise(r=>daemon.once('close',r));daemon.kill('SIGTERM');await stopped;}
  }
});

test('a slow native answer arrives as the service verdict before any client timeout',async t=>{
  const binary=process.env.COLLAB_TEST_BINARY;
  if(!binary){t.skip('Rust CLI consumer runs with COLLAB_TEST_BINARY');return;}
  // A host that answers read_thread slower than the plain socket budget but
  // well inside the service's native budget. Every outer budget is strictly
  // larger, so the client must receive the service's real rejection, never its
  // own timeout.
  const delayMs=6000;
  const root=await fs.mkdtemp('/tmp/cdr-'),state=path.join(root,'state');
  await fs.mkdir(state,{mode:0o700});
  const streams=new Set();
  const server=net.createServer(s=>{streams.add(s);s.on('close',()=>streams.delete(s));decode(s,r=>{
    if(r.method==='tools/list'){s.write(frame({id:r.id,result:{tools:['read_thread','send_message_to_thread'].map(name=>({name,namespace:'codex_app'}))}}));return;}
    const p=r.params;
    if(p.tool==='read_thread')setTimeout(()=>s.write(frame({id:r.id,result:{success:false,contentItems:[]}})),delayMs);
    else s.write(frame({id:r.id,result:{success:true,contentItems:[]}}));
  });});
  await new Promise(r=>server.listen(path.join(root,'host.sock'),r));
  t.after(async()=>{for(const s of streams)s.destroy();await new Promise(r=>server.close(r));await fs.rm(root,{recursive:true,force:true});});
  const mcp=path.join(path.dirname(binary),'collab-mcp');
  const env={PATH:process.env.PATH,COLLAB_STATE_DIR:state,CODEX_APP_TOOLS_PIPE_PATH:path.join(root,'host.sock'),
    CODEX_MCP_NODE_PATH:process.execPath,CODEX_INTERNAL_ORIGINATOR_OVERRIDE:'Codex',COLLAB_MCP_BIN:mcp,COLLAB_BIN:binary};
  const started=Date.now();
  const answer=await mcpContext(process.execPath,[script],{cwd:'/',env:{...env,CODEX_SESSION_ID:'foreign-session'}});
  const elapsed=Date.now()-started;
  assert.equal(answer.result.isError,true,JSON.stringify(answer));
  const text=answer.result.content[0].text;
  assert.match(text,/DESKTOP_BRIDGE_NATIVE_REJECTED/,text);
  assert.doesNotMatch(text,/service timeout/,text);
  assert.doesNotMatch(text,/managed child timeout/,text);
  assert.ok(elapsed>=delayMs,`the answer arrived after ${elapsed}ms, before the slow host replied`);
});

test('a silent bridge fails closed on its read bound instead of hanging the request',async t=>{
  const binary=process.env.COLLAB_TEST_BINARY;
  if(!binary){t.skip('Rust CLI consumer runs with COLLAB_TEST_BINARY');return;}
  // A bridge service socket that accepts the frame and never answers. The
  // adapter must end the request on its own bound with a typed timeout; it must
  // never wait forever, and it must not read a silence as a rejection.
  const {createHash}=await import('node:crypto');
  const root=await fs.mkdtemp('/tmp/cdr-'),state=path.join(root,'state'),runtime=path.join(state,'runtime','desktop');
  await fs.mkdir(runtime,{recursive:true,mode:0o700});
  const pipe=path.join(root,'host.sock');
  const silent=path.join(runtime,`pipe-${createHash('sha256').update(pipe).digest('hex').slice(0,24)}.sock`);
  const silentStreams=new Set();
  const server=net.createServer(s=>{silentStreams.add(s);s.on('close',()=>silentStreams.delete(s));});
  await new Promise(r=>server.listen(silent,r));
  t.after(async()=>{for(const s of silentStreams)s.destroy();await new Promise(r=>server.close(r));await fs.rm(root,{recursive:true,force:true});});
  const mcp=path.join(path.dirname(binary),'collab-mcp');
  const env={PATH:process.env.PATH,COLLAB_STATE_DIR:state,CODEX_APP_TOOLS_PIPE_PATH:pipe,CODEX_MCP_NODE_PATH:process.execPath,CODEX_INTERNAL_ORIGINATOR_OVERRIDE:'Codex',COLLAB_BIN:binary};
  const started=Date.now();
  const answer=await mcpContext(mcp,[],{cwd:'/',env,timeoutMs:120000});
  const elapsed=Date.now()-started;
  assert.equal(answer.result.isError,true,JSON.stringify(answer));
  assert.match(answer.result.content[0].text,/DESKTOP_BRIDGE_TIMEOUT/,answer.result.content[0].text);
  assert.ok(elapsed>=39000,`the silent bridge was abandoned after only ${elapsed}ms`);
  assert.ok(elapsed<90000,`the silent bridge held the request for ${elapsed}ms`);
});

test('daemon admits a legacy Desktop binding through a live publisher without host or node environment',async t=>{
  const binary=process.env.COLLAB_TEST_BINARY;
  if(!binary){t.skip('Rust CLI consumer runs with COLLAB_TEST_BINARY');return;}
  const f=await fixture(t),home=path.join(f.root,'home');await fs.mkdir(home);await fs.mkdir(path.join(f.root,'.agent-collab'));
  const baseEnv={...f.env,HOME:home,CODEX_HOME:home,COLLAB_DESKTOP_BRIDGE_SCRIPT:script};
  const daemonEnv={...baseEnv,CODEX_INTERNAL_ORIGINATOR_OVERRIDE:'Codex TUI'};
  delete daemonEnv.CODEX_APP_TOOLS_PIPE_PATH;delete daemonEnv.CODEX_MCP_NODE_PATH;
  let daemon,daemonLog='';
  function startDaemon(){daemon=spawn(binary,['serve'],{cwd:f.root,env:daemonEnv,stdio:['ignore','ignore','pipe']});daemon.stderr.on('data',b=>daemonLog+=b);}
  startDaemon();
  t.after(async()=>{if(daemon.exitCode===null){daemon.kill('SIGTERM');await new Promise(r=>daemon.once('exit',r));}});
  const socket=path.join(f.state,'server.sock');
  const started=Date.now()+10000;while(Date.now()<started){try{await fs.stat(socket);break;}catch{}if(daemon.exitCode!==null)throw Error('fixture daemon failed: '+daemonLog);await new Promise(r=>setTimeout(r,30));}
  async function cli(session,thread,environment){return await new Promise((resolve,reject)=>{const child=spawn(binary,['context'],{cwd:f.root,env:{...environment,CODEX_SESSION_ID:session,CODEX_THREAD_ID:thread},stdio:['ignore','pipe','pipe']});let out='',err='';child.stdout.on('data',b=>out+=b);child.stderr.on('data',b=>err+=b);child.on('exit',code=>code===0?resolve(out):reject(Error(out+err)));});}
  assert.match(await cli('legacy-a','thread-a',baseEnv),/committed|completed/);
  const cwd=await fs.realpath(f.root),current=capabilityPath(f.state,cwd,'legacy-a','thread-a');
  const published=JSON.parse(await fs.readFile(current,'utf8')).desktop_bridge;
  await f.stopService(published);
  // The legacy binding remains committed, but its enrollment and declaration are gone.
  await removeEnrollment(f.state,'legacy-a','thread-a');
  await fs.rm(path.dirname(current),{recursive:true,force:true});
  await assert.rejects(fs.readFile(current));
  // A different session starts the only live publisher. It has no A binding yet.
  const publisher=await f.admit('session-b','thread-b'),callsBefore=f.calls.length;
  let recovered;
  const deadline=Date.now()+10000;
  while(Date.now()<deadline){
    try{recovered=JSON.parse(await fs.readFile(current,'utf8')).desktop_bridge;break;}catch(error){if(error.code!=='ENOENT')throw error;}
    await new Promise(r=>setTimeout(r,50));
  }
  assert.ok(recovered,'daemon did not admit the legacy binding through the live publisher');
  assert.equal(recovered.endpoint,publisher.endpoint);
  assert.ok(f.calls.slice(callsBefore).some(c=>c.params?.tool==='read_thread'&&c.params?.threadId==='thread-a'),'publisher re-verifies the exact native thread');
  const noHost={...daemonEnv};delete noHost.CODEX_MCP_NODE_PATH;delete noHost.CODEX_APP_TOOLS_PIPE_PATH;
  assert.match(await cli('legacy-a','thread-a',noHost),/committed|completed/);
  const sent=await f.request(recovered,{id:'legacy',op:'send',session_id:'legacy-a',thread_id:'thread-a',text:'after publisher admission'});
  assert.equal(sent.ok,true);
});

test('daemon reports unsafe, malformed and stale publisher descriptors explicitly',async t=>{
  const binary=process.env.COLLAB_TEST_BINARY;
  if(!binary){t.skip('Rust CLI consumer runs with COLLAB_TEST_BINARY');return;}
  const f=await fixture(t),home=path.join(f.root,'home');await fs.mkdir(home);await fs.mkdir(path.join(f.root,'.agent-collab'));
  const baseEnv={...f.env,HOME:home,CODEX_HOME:home,COLLAB_DESKTOP_BRIDGE_SCRIPT:script};
  const daemonEnv={...baseEnv,CODEX_INTERNAL_ORIGINATOR_OVERRIDE:'Codex TUI'};
  delete daemonEnv.CODEX_APP_TOOLS_PIPE_PATH;delete daemonEnv.CODEX_MCP_NODE_PATH;
  let daemon,daemonLog='';
  function startDaemon(){daemon=spawn(binary,['serve'],{cwd:f.root,env:daemonEnv,stdio:['ignore','ignore','pipe']});daemon.stderr.on('data',b=>daemonLog+=b);}
  startDaemon();
  t.after(async()=>{if(daemon.exitCode===null){daemon.kill('SIGTERM');await new Promise(r=>daemon.once('exit',r));}});
  const socket=path.join(f.state,'server.sock');
  const started=Date.now()+10000;while(Date.now()<started){try{await fs.stat(socket);break;}catch{}if(daemon.exitCode!==null)throw Error('fixture daemon failed: '+daemonLog);await new Promise(r=>setTimeout(r,30));}
  async function cli(session,thread,environment){return await new Promise((resolve,reject)=>{const child=spawn(binary,['context'],{cwd:f.root,env:{...environment,CODEX_SESSION_ID:session,CODEX_THREAD_ID:thread},stdio:['ignore','pipe','pipe']});let out='',err='';child.stdout.on('data',b=>out+=b);child.stderr.on('data',b=>err+=b);child.on('exit',code=>code===0?resolve(out):reject(Error(out+err)));});}
  assert.match(await cli('legacy-a','thread-a',baseEnv),/committed|completed/);
  const cwd=await fs.realpath(f.root),current=capabilityPath(f.state,cwd,'legacy-a','thread-a');
  const published=JSON.parse(await fs.readFile(current,'utf8')).desktop_bridge;
  await f.stopService(published);
  await removeEnrollment(f.state,'legacy-a','thread-a');
  await fs.rm(path.dirname(current),{recursive:true,force:true});
  const services=path.join(f.state,'runtime','desktop','services'),file=path.join(services,'probe.json');
  let mark=0;
  async function waitLog(needle){const deadline=Date.now()+7000;while(Date.now()<deadline){const fileLog=await fs.readFile(path.join(f.state,'log.txt'),'utf8').catch(()=> '');const text=daemonLog+'\n'+fileLog;if(text.slice(mark).includes(needle)){mark=text.length;return;}await new Promise(r=>setTimeout(r,30));}const fileLog=await fs.readFile(path.join(f.state,'log.txt'),'utf8').catch(()=> '');throw Error(`missing ${needle}: ${(daemonLog+'\n'+fileLog).slice(mark)}`);}
  const dead=`unix://${path.join(f.state,'runtime','desktop','pipe-dead.sock')}`;
  await fs.writeFile(file,JSON.stringify({schema:'collab-desktop-runtime/v1',runtime_key:'dead',endpoint:dead,namespace:'codex_app',publisher_id:'probe'}),{mode:0o644});
  await waitLog('DECLARATION_UNSAFE');
  await fs.chmod(file,0o600);await fs.writeFile(file,'{');
  await waitLog('RUNTIME_DESCRIPTOR_INVALID');
  await fs.writeFile(file,JSON.stringify({schema:'collab-desktop-runtime/v1',runtime_key:'dead',endpoint:dead,namespace:'codex_app',publisher_id:'probe'}));
  await waitLog('stale or unreachable');
  await assert.rejects(fs.readFile(current));
  // Ambiguity must be detected before admission can publish a new scope.
  let admissions=0;
  process.kill(daemon.pid,'SIGSTOP');
  try{
    await fs.rm(file);
    for(const key of ['first','second']){
      const endpoint=path.join(f.state,'runtime','desktop',`pipe-${key}.sock`);
      const server=net.createServer(s=>decode(s,r=>{if(r.op==='admit')admissions++;s.end(frame({id:r.id,ok:true,runtime_key:key}));}));
      await new Promise(r=>server.listen(endpoint,r));
      t.after(()=>new Promise(r=>server.close(r)));
      await fs.writeFile(path.join(services,`${key}.json`),JSON.stringify({schema:'collab-desktop-runtime/v1',runtime_key:key,endpoint:`unix://${endpoint}`,namespace:'codex_app',publisher_id:key}),{mode:0o600});
    }
  }finally{process.kill(daemon.pid,'SIGCONT');}
  await waitLog('RUNTIME_AMBIGUOUS');
  assert.equal(admissions,0,'ambiguous publishers must not mutate an unbound scope before refusal');
  await assert.rejects(fs.readFile(current));
});

test('daemon does not treat a dead same-endpoint declaration as live',async t=>{
  const binary=process.env.COLLAB_TEST_BINARY;
  if(!binary){t.skip('Rust CLI consumer runs with COLLAB_TEST_BINARY');return;}
  const f=await fixture(t),home=path.join(f.root,'home');await fs.mkdir(home);await fs.mkdir(path.join(f.root,'.agent-collab'));
  const baseEnv={...f.env,HOME:home,CODEX_HOME:home,COLLAB_DESKTOP_BRIDGE_SCRIPT:script};
  const daemonEnv={...baseEnv,CODEX_INTERNAL_ORIGINATOR_OVERRIDE:'Codex TUI'};
  delete daemonEnv.CODEX_APP_TOOLS_PIPE_PATH;delete daemonEnv.CODEX_MCP_NODE_PATH;
  let daemon,daemonLog='';
  function startDaemon(){daemon=spawn(binary,['serve'],{cwd:f.root,env:daemonEnv,stdio:['ignore','ignore','pipe']});daemon.stderr.on('data',b=>daemonLog+=b);}
  startDaemon();
  t.after(async()=>{if(daemon.exitCode===null){daemon.kill('SIGTERM');await new Promise(r=>daemon.once('exit',r));}});
  const socket=path.join(f.state,'server.sock');
  const started=Date.now()+10000;while(Date.now()<started){try{await fs.stat(socket);break;}catch{}if(daemon.exitCode!==null)throw Error('fixture daemon failed: '+daemonLog);await new Promise(r=>setTimeout(r,30));}
  async function cli(session,thread,environment){return await new Promise((resolve,reject)=>{const child=spawn(binary,['context'],{cwd:f.root,env:{...environment,CODEX_SESSION_ID:session,CODEX_THREAD_ID:thread},stdio:['ignore','pipe','pipe']});let out='',err='';child.stdout.on('data',b=>out+=b);child.stderr.on('data',b=>err+=b);child.on('exit',code=>code===0?resolve(out):reject(Error(out+err)));});}
  assert.match(await cli('legacy-a','thread-a',baseEnv),/committed|completed/);
  const cwd=await fs.realpath(f.root),current=capabilityPath(f.state,cwd,'legacy-a','thread-a');
  const published=JSON.parse(await fs.readFile(current,'utf8')).desktop_bridge;
  await f.stopService(published);
  const logPath=path.join(f.state,'log.txt');let mark=(await fs.readFile(logPath,'utf8').catch(()=> '')).length,seen='';
  const deadline=Date.now()+7000;
  while(Date.now()<deadline){seen=(await fs.readFile(logPath,'utf8').catch(()=> '')).slice(mark);if(seen.includes('DESKTOP_BRIDGE_NOT_SUBMITTED'))break;await new Promise(r=>setTimeout(r,30));}
  assert.match(seen,/BINDING_PENDING:.*DESKTOP_BRIDGE_NOT_SUBMITTED/);
  assert.equal(JSON.parse(await fs.readFile(current,'utf8')).desktop_bridge.endpoint,published.endpoint);
});
