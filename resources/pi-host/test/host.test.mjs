import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawn } from 'node:child_process';
import readline from 'node:readline';
import http from 'node:http';
import { once } from 'node:events';
import { fileURLToPath } from 'node:url';
import { searchExa, searchBaidu } from '../web-tools.mjs';

/** The driver exercises the public JSONL boundary; server gates replace timed waits. */
function driver(child) {
  const pending = new Map();
  const events = [];
  const waiters = new Map();
  let request = 0;
  let diagnostic = '';
  child.stderr.on('data', data => { diagnostic += data; });
  readline.createInterface({ input: child.stdout }).on('line', line => {
    const value = JSON.parse(line);
    const response = pending.get(value.requestId);
    if (value.type === 'response' && response) {
      pending.delete(value.requestId);
      response.resolve({ ...value, diagnostic });
    } else {
      events.push(value);
      const id = `${value.sessionId}:${value.type}`;
      waiters.get(id)?.resolve(value);
      waiters.delete(id);
      const subtype = `${id}:${value.assistantMessageEvent?.type ?? value.toolCallId}`;
      waiters.get(subtype)?.resolve(value);
      waiters.delete(subtype);
    }
  });
  child.on('exit', () => {
    pending.forEach(waiter => waiter.reject(new Error(diagnostic)));
    waiters.forEach(waiter => waiter.reject(new Error(diagnostic)));
  });
  return {
    request: command => {
      const requestId = `test-${++request}`;
      const deferred = Promise.withResolvers();
      pending.set(requestId, deferred);
      child.stdin.write(`${JSON.stringify({ routeId: 'test', ...command, requestId })}\n`);
      return deferred.promise;
    },
    event: (sessionId, type, subtype) => {
      const existing = events.find(value => value.sessionId === sessionId && value.type === type
        && (subtype === undefined || (value.assistantMessageEvent?.type ?? value.toolCallId) === subtype));
      if (existing) return Promise.resolve(existing);
      const deferred = Promise.withResolvers();
      waiters.set(`${sessionId}:${type}${subtype === undefined ? '' : `:${subtype}`}`, deferred);
      return deferred.promise;
    },
  };
}

test('annotation tools cross the real SDK and JSONL request/reply boundary', { timeout: 45000 }, async t => {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'folio-annotation-host-'));
  const agentDir = path.join(directory, 'agent');
  const cwd = path.join(directory, 'project');
  fs.mkdirSync(agentDir); fs.mkdirSync(cwd);
  const requests = [];
  const server = http.createServer(async (request, response) => {
    const chunks = [];
    request.on('data', chunk => chunks.push(chunk));
    await once(request, 'end');
    const body = JSON.parse(Buffer.concat(chunks).toString());
    requests.push(body);
    const role = body.messages.at(-1).role;
    const delta = { user: { tool_calls:[{index:0, id:'add-note', type:'function', function:{name:'folio_annotation_create',
      arguments:JSON.stringify({path:'note.md', quote:'target', comment:'解释'})}}] }, tool:{content:'批注已添加'} }[role];
    response.setHeader('Content-Type', 'text/event-stream');
    response.write(`data: ${JSON.stringify({id:'annotation-run', choices:[{index:0, delta, finish_reason:null}]})}\n\n`);
    response.end(`data: ${JSON.stringify({id:'annotation-run', choices:[{index:0, delta:{}, finish_reason:{user:'tool_calls', tool:'stop'}[role]}]})}\n\ndata: [DONE]\n\n`);
  });
  server.listen(0, '127.0.0.1'); await once(server, 'listening');
  t.after(() => { server.closeAllConnections(); server.close(); });
  const baseUrl = `http://127.0.0.1:${server.address().port}`;
  fs.writeFileSync(path.join(agentDir, 'models.json'), JSON.stringify({providers:{magpie:{
    baseUrl:`${baseUrl}/v1`, api:'openai-completions', apiKey:'magpie', models:[{id:'test-model', name:'test-model',
      reasoning:false, input:['text'], contextWindow:16000, maxTokens:1024,
      cost:{input:0, output:0, cacheRead:0, cacheWrite:0}}],
  }}}));
  fs.writeFileSync(path.join(agentDir, 'settings.json'), JSON.stringify({defaultProvider:'magpie', defaultModel:'test-model'}));
  const child = spawn(process.execPath, [fileURLToPath(new URL('../host.mjs', import.meta.url))], {
    env:{...process.env, PI_CODING_AGENT_DIR:agentDir, FOLIO_USER_DATA:directory,
      FOLIO_AI_SETTINGS:JSON.stringify({baseUrl, titleModelId:''})}, windowsHide:true,
  });
  t.after(async () => {
    const exited = once(child, 'exit');
    child.stdin.end(); child.kill(); await exited;
    fs.rmSync(directory, {recursive:true, force:true});
  });
  const client = driver(child);
  const project = {cwd, projectKey:fs.realpathSync.native(cwd).replaceAll('/', '\\').replace(/\\+$/, '').toLowerCase()};
  const created = await client.request({...project, type:'create_session'});
  const sessionId = created.data.sessionId;
  // Prompt replies only after the run settles; service the UI request while that run is pending.
  const run = client.request({...project, sessionId, type:'prompt', message:'给 target 加批注'});
  const pending = await client.event(sessionId, 'annotation_request');
  assert.equal(pending.projectKey, project.projectKey);
  assert.equal(pending.params.quote, 'target');
  const result = {status:'ok', created:true, annotation:{id:42, path:'note.md', quote:'target', comment:'解释'}};
  assert.equal((await client.request({...project, sessionId, type:'annotation_response', id:pending.id, result})).success, true);
  const ended = await client.event(sessionId, 'tool_execution_end');
  assert.equal(ended.isError, false);
  assert.deepEqual(JSON.parse(ended.result.content[0].text), result);
  await client.event(sessionId, 'agent_settled');
  assert.equal((await run).success, true);
  assert.match(JSON.stringify(requests[0].messages), /folio-annotations/);
  assert.deepEqual(JSON.parse(requests[1].messages.at(-1).content), result);
});

test('independent SDK sessions keep running across snapshots, closing tabs, and manual rename', { timeout: 45000 }, async t => {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'folio-sdk-'));
  t.after(() => fs.rmSync(directory, { recursive: true, force: true }));
  const agentDir = path.join(directory, 'agent');
  const projectA = path.join(directory, 'a');
  const projectB = path.join(directory, 'b');
  fs.mkdirSync(agentDir); fs.mkdirSync(projectA); fs.mkdirSync(projectB);
  fs.mkdirSync(path.join(agentDir, 'extensions'));
  fs.writeFileSync(path.join(agentDir, 'extensions', 'approval.ts'), `
    export default function(pi) {
      pi.registerCommand('confirm-test', { description: 'Verify isolated dialogs', handler: async (_,ctx) => {
        await ctx.ui.confirm('First confirmation', ctx.cwd);
        await ctx.ui.confirm('Second confirmation', ctx.cwd);
      } });
    }
  `);
  const streamed = Promise.withResolvers();
  const titleRequested = Promise.withResolvers();
  const finishStream = Promise.withResolvers();
  const finishTitle = Promise.withResolvers();
  t.after(() => { finishStream.resolve(); finishTitle.resolve(); });
  const server = http.createServer(async (request, response) => {
    const chunks = [];
    request.on('data', chunk => chunks.push(chunk));
    await once(request, 'end');
    const body = JSON.parse(Buffer.concat(chunks).toString());
    if (!body.stream) {
      titleRequested.resolve();
      await finishTitle.promise;
      response.setHeader('Content-Type', 'application/json');
      response.end(JSON.stringify({ id: 'title', object: 'chat.completion', choices: [{ message: { role: 'assistant', content: '自动标题' }, finish_reason: 'stop' }] }));
      return;
    }
    response.setHeader('Content-Type', 'text/event-stream');
    const label = JSON.stringify(body.messages).includes('second user message') ? '乙' : '甲';
    response.write(`data: ${JSON.stringify({ id: 'run', choices: [{ index: 0, delta: { role: 'assistant', content: `${label}开始` }, finish_reason: null }] })}\n\n`);
    streamed.resolve();
    await finishStream.promise;
    response.write(`data: ${JSON.stringify({ id: 'run', choices: [{ index: 0, delta: { content: `${label}完成` }, finish_reason: null }] })}\n\n`);
    response.write(`data: ${JSON.stringify({ id: 'run', choices: [{ index: 0, delta: {}, finish_reason: 'stop' }] })}\n\n`);
    response.end('data: [DONE]\n\n');
  });
  server.listen(0, '127.0.0.1'); await once(server, 'listening');
  t.after(() => { server.closeAllConnections(); server.close(); });
  const base = `http://127.0.0.1:${server.address().port}`;
  fs.writeFileSync(path.join(agentDir, 'models.json'), JSON.stringify({ providers: { magpie: {
    baseUrl: `${base}/v1`, api: 'openai-completions', apiKey: 'magpie', models: [{ id: 'test-model', name: 'test-model',
      reasoning: false, input: ['text'], contextWindow: 16000, maxTokens: 1024,
      cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 } }],
  } } }));
  fs.writeFileSync(path.join(agentDir, 'settings.json'), JSON.stringify({ defaultProvider: 'magpie', defaultModel: 'test-model' }));
  const child = spawn(process.execPath, [fileURLToPath(new URL('../host.mjs', import.meta.url))], {
    env: { ...process.env, PI_CODING_AGENT_DIR: agentDir, FOLIO_USER_DATA: directory,
      FOLIO_AI_SETTINGS: JSON.stringify({ baseUrl: base, titleModelId: 'test-model' }) }, windowsHide: true,
  });
  t.after(() => { child.stdin.end(); child.kill(); });
  const client = driver(child);
  const projectKey = folder => fs.realpathSync.native(folder).replaceAll('/', '\\').replace(/\\+$/, '').toLowerCase();
  const a = { cwd: projectA, projectKey: projectKey(projectA) };
  const b = { cwd: projectB, projectKey: projectKey(projectB) };
  const createdA = await client.request({ ...a, type: 'create_session' });
  assert.equal(createdA.success, true, createdA.diagnostic || createdA.error);
  const sessionId = createdA.data.sessionId;
  const createdB = await client.request({ ...b, type: 'create_session' });
  assert.equal(createdB.success, true, createdB.error);
  const run = client.request({ ...a, sessionId, type: 'prompt', message: 'first user message', titleText: 'first user message' });
  await streamed.promise; await titleRequested.promise;
  const snapshotB = await client.request({ ...b, sessionId: createdB.data.sessionId, type: 'snapshot' });
  assert.deepEqual(snapshotB.data.messages, []);
  const runB = client.request({ ...b, sessionId: createdB.data.sessionId, type: 'prompt', message: 'second user message' });
  await client.event(createdB.data.sessionId, 'message_update');
  const snapshotA = await client.request({ ...a, sessionId, type: 'snapshot' });
  assert.equal(snapshotA.data.isStreaming, true);
  assert.match(JSON.stringify(snapshotA.data.messages), /甲开始/);
  const protectedDelete = await client.request({ ...a, sessionId, type: 'delete_session' });
  assert.equal(protectedDelete.success, false);
  const rename = await client.request({ ...a, sessionId, type: 'set_session_name', name: '手动标题' });
  assert.equal(rename.success, true);
  const released = await client.request({ ...a, sessionId, type: 'release' });
  assert.equal(released.success, true);
  const retained = await client.request({ ...a, sessionId, type: 'retain' });
  assert.equal(retained.success, true);
  const releasedB = await client.request({ ...b, sessionId: createdB.data.sessionId, type: 'release' });
  assert.equal(releasedB.success, true);
  const denied = await client.request({ ...b, sessionId, type: 'snapshot' });
  assert.equal(denied.success, false);
  finishTitle.resolve(); finishStream.resolve();
  assert.equal((await run).success, true);
  assert.equal((await runB).success, true);
  await client.event(sessionId, 'agent_settled');
  const reopened = await client.request({ ...a, sessionId, type: 'snapshot' });
  assert.equal(reopened.success, true, reopened.error);
  assert.equal(reopened.data.sessionName, '手动标题');
  assert.equal(reopened.data.isStreaming, false);
  assert.match(JSON.stringify(reopened.data.messages), /甲开始甲完成/);
  assert.doesNotMatch(JSON.stringify(reopened.data.messages), /乙开始/);
  const completedB = await client.request({ ...b, sessionId: createdB.data.sessionId, type: 'snapshot' });
  assert.match(JSON.stringify(completedB.data.messages), /乙开始乙完成/);
  assert.doesNotMatch(JSON.stringify(completedB.data.messages), /甲开始/);
  const approvalA = client.request({ ...a, sessionId, type: 'prompt', message: '/confirm-test' });
  const approvalB = client.request({ ...b, sessionId: createdB.data.sessionId, type: 'prompt', message: '/confirm-test' });
  const dialogA = await client.event(sessionId, 'extension_ui_request');
  const dialogB = await client.event(createdB.data.sessionId, 'extension_ui_request');
  assert.equal(dialogA.method, 'confirm');
  assert.equal(dialogB.method, 'confirm');
  assert.notEqual(dialogA.id, dialogB.id);
  assert.equal((await client.request({ ...a, sessionId, type: 'extension_ui_response', id: dialogA.id, confirmed: true })).success, true);
  const waitingB = await client.request({ ...b, sessionId: createdB.data.sessionId, type: 'get_state' });
  assert.equal(waitingB.data.waiting, true);
  const deletingB = await client.request({ ...b, sessionId: createdB.data.sessionId, type: 'delete_session', confirmed: true });
  assert.equal(deletingB.success, true);
  assert.equal((await approvalB).success, true);
  const stoppedA = await client.request({ ...a, sessionId, type: 'abort' });
  assert.equal(stoppedA.success, true);
  assert.equal((await approvalA).success, true);
  const pinned = await client.request({ ...a, sessionId, type: 'set_pinned', pinned: true });
  assert.equal(pinned.success, true);
  const listed = await client.request({ ...a, type: 'list_sessions' });
  assert.equal(listed.data.sessions[0].pinned, true);
  assert.equal(listed.data.sessions[0].id, sessionId);
  const removed = await client.request({ ...a, sessionId, type: 'delete_session', confirmed: true });
  assert.equal(removed.success, true);
  const afterDelete = await client.request({ ...a, type: 'list_sessions' });
  assert.deepEqual(afterDelete.data.sessions, []);
});

test('creating a tab returns its identity before model initialization completes', { timeout: 45000 }, async t => {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'folio-new-tab-'));
  const agentDir = path.join(directory, 'agent');
  const cwd = path.join(directory, 'project');
  fs.mkdirSync(agentDir); fs.mkdirSync(cwd);
  fs.writeFileSync(path.join(agentDir, 'settings.json'), JSON.stringify({ defaultProvider: 'magpie', defaultModel: 'test-model' }));
  const discovery = Promise.withResolvers();
  const ready = Promise.withResolvers();
  t.after(() => ready.resolve());
  const server = http.createServer(async (_request, response) => {
    discovery.resolve();
    await ready.promise;
    response.setHeader('Content-Type', 'application/json');
    response.end(JSON.stringify({ data: [{ id: 'test-model' }] }));
  });
  server.listen(0, '127.0.0.1'); await once(server, 'listening');
  t.after(() => { server.closeAllConnections(); server.close(); });
  const child = spawn(process.execPath, [fileURLToPath(new URL('../host.mjs', import.meta.url))], {
    env: { ...process.env, PI_CODING_AGENT_DIR: agentDir, FOLIO_USER_DATA: directory,
      FOLIO_AI_SETTINGS: JSON.stringify({ baseUrl: `http://127.0.0.1:${server.address().port}`, titleModelId: '' }) }, windowsHide: true,
  });
  t.after(async () => {
    const exited = once(child, 'exit');
    child.stdin.end(); child.kill();
    await exited;
    fs.rmSync(directory, { recursive: true, force: true });
  });
  const client = driver(child);
  const project = { cwd, projectKey: fs.realpathSync.native(cwd).replaceAll('/', '\\').replace(/\\+$/, '').toLowerCase() };
  const created = await client.request({ ...project, type: 'create_session' });
  assert.equal(created.success, true, created.error);
  const sessionId = created.data.sessionId;
  assert.equal(typeof sessionId, 'string');
  const snapshot = client.request({ ...project, sessionId, type: 'snapshot' });
  await discovery.promise;
  const listed = await client.request({ ...project, type: 'list_sessions' });
  assert.equal(listed.data.sessions[0].id, sessionId);
  ready.resolve();
  const loaded = await snapshot;
  assert.equal(loaded.success, true, loaded.error);
  assert.equal(loaded.data.sessionId, sessionId);
  assert.deepEqual(loaded.data.messages, []);
});

test('tool argument deltas reach the conversation before generation ends', { timeout: 45000 }, async t => {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'folio-tool-stream-'));
  t.after(() => fs.rmSync(directory, { recursive: true, force: true }));
  const agentDir = path.join(directory, 'agent');
  const cwd = path.join(directory, 'project');
  fs.mkdirSync(agentDir); fs.mkdirSync(cwd);
  const finish = Promise.withResolvers();
  t.after(() => finish.resolve());
  const server = http.createServer(async (request, response) => {
    request.resume(); await once(request, 'end');
    response.setHeader('Content-Type', 'text/event-stream');
    response.write(`data: ${JSON.stringify({ id: 'tool-run', choices: [{ index: 0, delta: { role: 'assistant', content: '先检查文件。' }, finish_reason: null }] })}\n\n`);
    response.write(`data: ${JSON.stringify({ id: 'tool-run', choices: [{ index: 0, delta: { tool_calls: [{ index: 0, id: 'call-read', type: 'function', function: { name: 'read', arguments: '{"path":"README' } }] }, finish_reason: null }] })}\n\n`);
    await finish.promise;
    response.end();
  });
  server.listen(0, '127.0.0.1'); await once(server, 'listening');
  t.after(() => { server.closeAllConnections(); server.close(); });
  const baseUrl = `http://127.0.0.1:${server.address().port}`;
  fs.writeFileSync(path.join(agentDir, 'models.json'), JSON.stringify({ providers: { magpie: {
    baseUrl: `${baseUrl}/v1`, api: 'openai-completions', apiKey: 'magpie', models: [{ id: 'test-model', name: 'test-model',
      reasoning: false, input: ['text'], contextWindow: 16000, maxTokens: 1024,
      cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 } }],
  } } }));
  fs.writeFileSync(path.join(agentDir, 'settings.json'), JSON.stringify({ defaultProvider: 'magpie', defaultModel: 'test-model' }));
  const child = spawn(process.execPath, [fileURLToPath(new URL('../host.mjs', import.meta.url))], {
    env: { ...process.env, PI_CODING_AGENT_DIR: agentDir, FOLIO_USER_DATA: directory,
      FOLIO_AI_SETTINGS: JSON.stringify({ baseUrl, titleModelId: '' }) }, windowsHide: true,
  });
  t.after(() => { child.stdin.end(); child.kill(); });
  const client = driver(child);
  const project = { cwd, projectKey: fs.realpathSync.native(cwd).replaceAll('/', '\\').replace(/\\+$/, '').toLowerCase() };
  const created = await client.request({ ...project, type: 'create_session' });
  const sessionId = created.data.sessionId;
  const run = client.request({ ...project, sessionId, type: 'prompt', message: '检查文件' });
  const delta = await client.event(sessionId, 'message_update', 'toolcall_delta');
  assert.equal(delta.assistantMessageEvent.partial.content[1].name, 'read');
  assert.match(delta.assistantMessageEvent.partial.content[1].arguments.path, /README/);
  const snapshot = await client.request({ ...project, sessionId, type: 'snapshot' });
  assert.equal(snapshot.data.isStreaming, true);
  assert.equal(snapshot.data.messages.at(-1).content[1].name, 'read');
  const stopped = await client.request({ ...project, sessionId, type: 'abort' });
  assert.equal(stopped.success, true);
  assert.equal((await run).success, true);
});

test('writes retain original content and runtime clock snapshots preserve the request prefix', { timeout: 45000 }, async t => {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'folio-write-diff-'));
  const agentDir = path.join(directory, 'agent');
  const cwd = path.join(directory, 'project');
  fs.mkdirSync(agentDir); fs.mkdirSync(cwd);
  const requests = [];
  const server = http.createServer(async (request, response) => {
    const chunks = [];
    request.on('data', chunk => chunks.push(chunk));
    await once(request, 'end');
    const body = JSON.parse(Buffer.concat(chunks).toString());
    requests.push(body);
    response.setHeader('Content-Type', 'text/event-stream');
    const role = body.messages.at(-1).role;
    const content = body.messages.findLast(message => message.role === 'user' && message.content[0].text.startsWith('# ')).content[0].text;
    const toolCallId = { '# 新文件\n第一行\n第二行\n': 'write-new', '# 覆盖\n修改后\n': 'write-overwrite' }[content];
    const delta = { tool: { content: '完成' }, user: { tool_calls: [{ index: 0, id: toolCallId, type: 'function',
      function: { name: 'write', arguments: JSON.stringify({ path: '新建文件.md', content }) } }] } }[role];
    response.write(`data: ${JSON.stringify({ id: 'write-run', choices: [{ index: 0, delta, finish_reason: null }] })}\n\n`);
    response.end(`data: ${JSON.stringify({ id: 'write-run', choices: [{ index: 0, delta: {}, finish_reason: { tool: 'stop', user: 'tool_calls' }[role] }] })}\n\ndata: [DONE]\n\n`);
  });
  server.listen(0, '127.0.0.1'); await once(server, 'listening');
  t.after(() => { server.closeAllConnections(); server.close(); });
  const baseUrl = `http://127.0.0.1:${server.address().port}`;
  fs.writeFileSync(path.join(agentDir, 'models.json'), JSON.stringify({ providers: { magpie: {
    baseUrl: `${baseUrl}/v1`, api: 'openai-completions', apiKey: 'magpie', models: [{ id: 'test-model', name: 'test-model',
      reasoning: false, input: ['text'], contextWindow: 16000, maxTokens: 1024,
      cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 } }],
  } } }));
  fs.writeFileSync(path.join(agentDir, 'settings.json'), JSON.stringify({ defaultProvider: 'magpie', defaultModel: 'test-model' }));
  const child = spawn(process.execPath, [fileURLToPath(new URL('../host.mjs', import.meta.url))], {
    env: { ...process.env, PI_CODING_AGENT_DIR: agentDir, FOLIO_USER_DATA: directory,
      FOLIO_AI_SETTINGS: JSON.stringify({ baseUrl, titleModelId: '' }) }, windowsHide: true,
  });
  t.after(async () => {
    const exited = once(child, 'exit');
    child.stdin.end(); child.kill();
    await exited;
    fs.rmSync(directory, { recursive: true, force: true });
  });
  const client = driver(child);
  const project = { cwd, projectKey: fs.realpathSync.native(cwd).replaceAll('/', '\\').replace(/\\+$/, '').toLowerCase() };
  const created = await client.request({ ...project, type: 'create_session' });
  const sessionId = created.data.sessionId;
  assert.equal((await client.request({ ...project, sessionId, type: 'prompt', message: '# 新文件\n第一行\n第二行\n' })).success, true);
  await client.event(sessionId, 'agent_settled');
  const writeResult = await client.event(sessionId, 'tool_execution_end');
  assert.equal(writeResult.isError, false, JSON.stringify(writeResult));
  // Inspect after writing has completed, as a delayed UI consumer would.
  assert.equal(fs.readFileSync(path.join(cwd, '新建文件.md'), 'utf8'), '# 新文件\n第一行\n第二行\n');
  assert.equal((await client.event(sessionId, 'tool_execution_start')).writeBefore, '');
  assert.equal((await client.request({ ...project, sessionId, type: 'prompt', message: '# 覆盖\n修改后\n' })).success, true);
  assert.equal(fs.readFileSync(path.join(cwd, '新建文件.md'), 'utf8'), '# 覆盖\n修改后\n');
  assert.equal((await client.event(sessionId, 'tool_execution_start', 'write-overwrite')).writeBefore, '# 新文件\n第一行\n第二行\n');
  assert.equal((await client.event(sessionId, 'tool_execution_end', 'write-overwrite')).isError, false);
  const history = await client.request({ ...project, sessionId, type: 'snapshot' });
  const clocks = history.data.messages.filter(message => message.customType === 'folio_runtime_context');
  assert.equal(clocks.length, 2);
  assert.equal(clocks[0].display, false);
  assert.equal(Number.isNaN(Date.parse(clocks[0].details.utc)), false);
  assert.equal(clocks[0].details.timeZone, Intl.DateTimeFormat().resolvedOptions().timeZone);
  assert.match(JSON.stringify(requests[0].messages), /你是 Folio 中的 AI 助手/);
  assert.match(JSON.stringify(requests[0].messages.at(-1)), /folio_runtime_context/);
  // Both a tool continuation and the next user run retain the exact previous request prefix.
  assert.deepEqual(requests[1].messages.slice(0, requests[0].messages.length), requests[0].messages);
  assert.deepEqual(requests[2].messages.slice(0, requests[1].messages.length), requests[1].messages);
  assert.match(JSON.stringify(requests[2].messages.at(-1)), /folio_runtime_context/);
});

test('Exa supports academic scope, focused evidence, domain filters and provider metadata', async () => {
  const transport = test.mock.fn(async () => ({ results: [
    { title: '论文', url: 'https://example.com/paper', publishedDate: '2026-11-01', highlights: ['关键证据'],
      author: '作者', highlightScores: [0.9], entities: [{ id: 'entity' }] },
    { title: '重复论文', url: 'https://example.com/paper', highlights: ['重复'] },
    { title: '无 URL 论文一', highlights: ['证据一'] }, { title: '无 URL 论文二', highlights: ['证据二'] },
  ] }));
  const result = await searchExa({ query: ' 查询 ', mode: 'academic', focus: ' 训练数据 ', recency: 'week',
    include_domains: ['Example.com'], exclude_domains: ['excluded.com'] }, { exaApiKey: 'exa-secret' },
  new AbortController().signal, transport);
  const [url, headers, body] = transport.mock.calls[0].arguments;
  assert.equal(url, 'https://api.exa.ai/search');
  assert.equal(headers['x-api-key'], 'exa-secret');
  assert.equal(body.query, '查询');
  assert.equal(body.category, 'research paper');
  assert.equal(body.numResults, 10);
  assert.deepEqual(body.contents, { highlights: { query: '训练数据' }, text: false, summary: false });
  assert.deepEqual(body.includeDomains, ['example.com']);
  assert.deepEqual(body.excludeDomains, ['excluded.com']);
  assert.match(body.startPublishedDate, /^\d{4}-\d{2}-\d{2}T/);
  assert.equal(result.mode, 'academic');
  assert.equal(result.candidates.length, 3);
  assert.deepEqual(result.candidates[0].evidences, ['关键证据']);
  assert.deepEqual(result.candidates[0].metadata, { author: '作者', highlight_scores: [0.9], entities: [{ id: 'entity' }] });
  assert.doesNotMatch(JSON.stringify(result), /secret/);
});

test('Baidu preserves site and Beijing-day filters and source evidence scores', async t => {
  const today = '2026-11-17';
  t.mock.timers.enable({ apis: ['Date'], now: new Date('2026-11-17T00:30:00+08:00') });
  const transport = test.mock.fn(async () => ({ references: [{ title: '来源', url: 'https://example.com',
    markdown_content: 'Markdown 证据', content: '备用正文', snippet: '片段', authority_score: 0, rerank_score: 0.9, aladdin: { type: 'news' } }] }));
  const result = await searchBaidu({ query: '查询', recency: 'day', include_domains: ['example.com'] },
    { baiduQianfanApiKey: 'baidu-secret' }, new AbortController().signal, transport);
  const [url, headers, body] = transport.mock.calls[0].arguments;
  assert.equal(url, 'https://qianfan.baidubce.com/v2/ai_search/web_search');
  assert.equal(headers['X-Appbuilder-Authorization'], 'Bearer baidu-secret');
  assert.equal(result.provider, 'baidu_qianfan');
  assert.deepEqual(result.candidates[0].evidences, ['Markdown 证据']);
  assert.deepEqual(result.candidates[0].metadata, { authority_score: 0, rerank_score: 0.9, aladdin: { type: 'news' } });
  assert.deepEqual(body.search_filter, { match: { site: ['example.com'] }, range: { page_time: { gte: today, lte: today } } });
  assert.equal(body.resource_type_filter[0].top_k, 10);
  assert.doesNotMatch(JSON.stringify(result), /secret/);
});

test('cancelling Exa stops the request without calling another provider', async () => {
  const controller = new AbortController();
  const transport = test.mock.fn(async () => { controller.abort(new Error('cancelled')); throw controller.signal.reason; });
  await assert.rejects(searchExa({ query: 'query' }, { exaApiKey: 'x', baiduQianfanApiKey: 'y' }, controller.signal, transport), /cancelled/);
  assert.equal(transport.mock.callCount(), 1);
});
