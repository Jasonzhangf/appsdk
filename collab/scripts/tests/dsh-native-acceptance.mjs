import assert from 'node:assert/strict';
import { once } from 'node:events';
import { spawn } from 'node:child_process';
import { createServer } from 'node:http';
import { chmod, mkdir, mkdtemp, readFile, realpath, rm, stat, symlink, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';

const args = process.argv.slice(2);
if (args.length !== 3) throw Error('Pass the candidate Collab binary, built DSH gateway checkout, and installed DSH launcher');
const [collab, gatewayRepo, dsh] = args.map(value => resolve(value));
const { DaemonDriver } = await import(join(gatewayRepo, 'e2e/daemon-driver.mjs'));
const { SdkClient } = await import(join(gatewayRepo, 'e2e/sdk-client.mjs'));
const root = await mkdtemp(join(await realpath(tmpdir()), 'cdsh-'));
const home = join(root, 'home');
const dshHome = join(root, 'dsh');
const profile = join(dshHome, 'profiles', 'collab-native');
const project = join(root, 'project');
const state = join(root, 'collab');
const socket = join(state, 'server.sock');
const sessions = join(root, 'sessions');
const bashResult = agent => join(root, `context-${agent}`);
const agentA = 'collab-native-a';
const agentB = 'collab-native-b';
const body = 'Collab native message\n第二行：完整正文';
let gateway, runtime, collabProcess, model, target;
let collabStderr = '';
let sentTool = false;
const registered = new Set();
let sawBody = false;
let statusMessage;
let statusTool = false;
let modelCalls = 0;

const wait = async (description, operation, deadline = 20_000) => {
  const end = Date.now() + deadline;
  do {
    const value = await operation();
    if (value) return value;
    await delay(100);
  } while (Date.now() < end);
  throw Error(description);
};
const hasSocket = async () => (await stat(socket).catch(() => undefined))?.isSocket();

try {
  await mkdir(join(profile, 'node_modules'), { recursive: true });
  await mkdir(join(project, '.agent-collab'), { recursive: true });
  await mkdir(home);
  await mkdir(state);
  await chmod(state, 0o700);
  const runtimePackages = join(dirname(dirname(await realpath(dsh))), 'node_modules/@deepseek-ai');
  await symlink(runtimePackages, join(profile, 'node_modules/@deepseek-ai'), 'dir');
  await symlink(join(gatewayRepo, 'packages/gateway'), join(profile, 'node_modules/dsh-gateway'), 'dir');
  await writeFile(join(profile, 'package.json'), JSON.stringify({
    name: 'dsh-profile-collab-native', private: true,
    dsh: { profile: { bundles: ['@deepseek-ai/dsh-sdk-minimal', 'dsh-gateway'] } }
  }));
  model = createServer(async (request, response) => {
    const chunks = [];
    for await (const chunk of request) chunks.push(chunk);
    const input = JSON.parse(Buffer.concat(chunks).toString());
    modelCalls++;
    const userText = JSON.stringify(input.messages.filter(message => message.role === 'user'));
    let call;
    const registering = [agentA, agentB].find(agent => userText.includes(`REGISTER-${agent}`) && !registered.has(agent));
    if (registering) {
      registered.add(registering);
      const bash = input.tools?.find(t => t.function.name.toLowerCase() === 'bash')?.function.name;
      if (bash) call = [bash, { command: `"${collab}" context > "${bashResult(registering)}"` }];
    } else if (userText.includes('SEND-TO-RECEIVER') && !sentTool) {
      sentTool = true;
      call = ['collab_send', { to: target, body, type: 'notify', subject: 'Real DSH canonical receipt' }];
    } else if (userText.includes('READ-SENT-STATUS') && !statusTool) {
      statusTool = true;
      call = ['collab_msg_status', { messageId: statusMessage }];
    }
    if (userText.includes('Collab native message') && userText.includes('完整正文')) sawBody = true;
    const base = { id: 'chatcmpl-collab-test', object: 'chat.completion.chunk', created: 1, model: 'test' };
    const chunk = (delta, finish_reason) => `data: ${JSON.stringify({ ...base, choices: [{ index: 0, delta, finish_reason }] })}\n\n`;
    response.writeHead(200, { 'Content-Type': 'text/event-stream' });
    response.write(chunk({ role: 'assistant' }, null));
    if (call) {
      const [name, args] = call;
      response.write(chunk({ tool_calls: [{ index: 0, id: `call_${modelCalls}`, type: 'function', function: { name, arguments: JSON.stringify(args) } }] }, null));
    } else {
      response.write(chunk({ content: 'READY' }, null));
    }
    response.end(chunk({}, call ? 'tool_calls' : 'stop') + 'data: [DONE]\n\n');
  });
  model.listen(0, '127.0.0.1');
  await once(model, 'listening');
  await writeFile(join(profile, 'cordis.patch.yml'), `
- insert:
    - id: credentials
      name: "@deepseek-ai/dsh-credentials-local"
    - id: llm-pi-ai
      name: "@deepseek-ai/dsh-llm-pi-ai"
      config:
        providers:
          loopback:
            apiKeyEnv: COLLAB_TEST_MODEL_KEY
            api: openai-completions
            baseURL: http://127.0.0.1:${model.address().port}/v1
            models:
              - id: test
                name: Test
                input: [text]
    - id: agent-default-model
      name: "@deepseek-ai/dsh-agent-default-model"
      config: { provider: loopback, model: test }
- id: llm-deepseek
  disabled: true
- id: sessions
  config:
    root: "${sessions}"
    compression: none
`);
  const cleanEnv = {
    HOME: home, PATH: [dirname(process.execPath), dirname(dsh), '/usr/bin', '/bin'].join(':'),
    COLLAB_STATE_DIR: state, DSH_HOME: dshHome,
  };
  collabProcess = spawn(collab, ['serve'], { cwd: project, env: cleanEnv, stdio: ['ignore', 'ignore', 'pipe'] });
  collabProcess.stderr.on('data', data => { collabStderr += data; });
  await wait('Collab private socket failed to start', hasSocket);
  const daemonDir = join(root, 'gateway');
  gateway = await DaemonDriver.spawnProcess({
    stateDir: daemonDir, controlSocket: join(daemonDir, 'control.sock'),
    token: 'collab-native-isolated-fixture', collab: { socketPath: socket },
    env: cleanEnv,
  });
  const daemonUrl = await wait('Gateway did not publish its loopback URL', () => {
    try { return gateway.daemonUrl(); } catch { return undefined; }
  });
  console.log('Private Collab and real gateway ready');
  runtime = new SdkClient({
    dshBin: dsh, profile: 'collab-native', cwd: project,
    env: { ...cleanEnv, DSH_GATEWAY_URL: daemonUrl,
      DSH_GATEWAY_DAEMON_TOKEN: 'collab-native-isolated-fixture',
      COLLAB_TEST_MODEL_KEY: 'loopback-fixture' },
  });
  runtime.start();
  await runtime.initialize({ cwd: project, provider: 'loopback', model: 'test' });
  await wait('DSH gateway connection did not become ready', async () => {
    const snapshot = await gateway.control.runtimes();
    return (snapshot.runtimes ?? snapshot).some(entry => entry.ready === true);
  });
  async function register(agent) {
    await runtime.prompt(agent, `REGISTER-${agent}`);
    await runtime.waitForStatus(agent, 'idle', { timeoutMs: 15_000 });
    const bootstrap = JSON.parse(await wait('DSH Bash did not return context', () => readFile(bashResult(agent), 'utf8').catch(() => '')));
    assert.equal(bootstrap.result?.outcome, 'completed');
    const context = bootstrap.result.snapshot;
    assert.equal(context.identity?.transport?.kind, 'dsh');
    assert.equal(context.session_id, agent);
    return context;
  }
  const receiver = await register(agentB);
  await register(agentA);
  target = receiver.identity?.worker_id ?? receiver.name;
  assert.equal(typeof target, 'string');
  console.log('Bare context registered both real DSH sessions', receiver.name);
  await runtime.prompt(agentA, 'SEND-TO-RECEIVER');
  const sent = await wait('No Collab send result from real DSH agent', () => runtime.events(agentA)
    .filter(event => event.type === 'tool/result')
    .map(event => JSON.stringify(event.data))
    .map(text => /collab message sent as this agent: ([^"\\ ]+)/.exec(text)?.[1])
    .find(Boolean));
  console.log('Sent', sent);
  const turn = await wait('No autonomous DSH native turn', () => runtime.events(agentB)
    .find(event => event.type === 'user/message' && event.data?.source?.messageId?.endsWith(`:${sent}`)));
  assert.ok(turn.data.source.messageId.startsWith('collab-poll:'));
  await wait('Canonical text did not reach the native model turn', () => sawBody);
  const gatewayAck = await wait('Gateway did not durably acknowledge the native delivery', async () => {
    const lines = await readFile(join(daemonDir, 'queue.jsonl'), 'utf8').catch(() => '');
    return lines.split('\n').filter(Boolean).map(line => JSON.parse(line))
      .find(record => record.t === 'ack' && record.messageId === turn.data.source.messageId);
  });
  statusMessage = sent;
  await runtime.prompt(agentA, 'READ-SENT-STATUS');
  const status = await wait('No consumed confirmation from native status tool', () => runtime.events(agentA)
    .filter(event => event.type === 'tool/result')
    .map(event => JSON.stringify(event.data))
    .find(text => /collab message state=read .* consumed=true /.test(text)));
  assert.ok(status);
  console.log('PASS', JSON.stringify({ messageId: sent, canonicalId: gatewayAck.messageId, nativeTurn: sawBody, gatewayAck: true, consumed: true }));
} catch (error) {
  console.error(error.message);
  console.error('Fixture diagnostics', JSON.stringify({
    modelCalls, registered: [...registered], sentTool, sawBody, statusTool,
    runtimeErrors: runtime?.stderrChunks.join('').split('\n').filter(line => /error|skip/i.test(line)).slice(-7),
    gatewayErrors: gateway?.stderr.slice(-3),
    collabError: collabStderr.slice(-700),
    aResults: runtime?.events(agentA).filter(e => e.type === 'tool/result').map(e => e.data),
    bResults: runtime?.events(agentB).filter(e => e.type === 'tool/result').map(e => e.data),
  }));
  process.exitCode = 1;
} finally {
  if (runtime) await runtime.close().catch(() => runtime.kill('SIGTERM'));
  if (gateway) await gateway.stop();
  if (collabProcess?.exitCode === null && collabProcess?.signalCode === null) {
    collabProcess.kill('SIGTERM');
    await once(collabProcess, 'exit');
  }
  model?.closeAllConnections();
  model?.close();
  await rm(root, { recursive: true, force: true });
}
