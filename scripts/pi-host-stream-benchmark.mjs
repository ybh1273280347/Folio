import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawn } from 'node:child_process';
import http from 'node:http';
import readline from 'node:readline';
import { once } from 'node:events';
import { fileURLToPath } from 'node:url';

const root = fs.mkdtempSync(path.join(os.tmpdir(), 'folio-pi-stream-bench-'));
const agentDir = path.join(root, 'agent');
const cwd = path.join(root, 'project');
fs.mkdirSync(agentDir);
fs.mkdirSync(cwd);

const rounds = Number(process.env.FOLIO_BENCH_ROUNDS ?? 5);
const chunkCount = Number(process.env.FOLIO_BENCH_CHUNKS ?? 64);
const chunkBytes = Number(process.env.FOLIO_BENCH_CHUNK_BYTES ?? 256);
const content = 'x'.repeat(chunkBytes);
const server = http.createServer(async (request, response) => {
  for await (const _chunk of request) {}
  response.writeHead(200, { 'Content-Type': 'text/event-stream' });
  for (let index = 0; index < chunkCount; index += 1) {
    response.write(`data: ${JSON.stringify({ id: 'benchmark', choices: [{ index: 0, delta: { content }, finish_reason: null }] })}\n\n`);
  }
  response.end(`data: ${JSON.stringify({ id: 'benchmark', choices: [{ index: 0, delta: {}, finish_reason: 'stop' }] })}\n\ndata: [DONE]\n\n`);
});
server.listen(0, '127.0.0.1');
await once(server, 'listening');
const baseUrl = `http://127.0.0.1:${server.address().port}`;
fs.writeFileSync(path.join(agentDir, 'models.json'), JSON.stringify({ providers: { magpie: {
  baseUrl: `${baseUrl}/v1`, api: 'openai-completions', apiKey: 'benchmark', models: [{ id: 'benchmark-model', name: 'benchmark-model',
    reasoning: false, input: ['text'], contextWindow: 200000, maxTokens: 32768,
    cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 } }],
} } }));
fs.writeFileSync(path.join(agentDir, 'settings.json'), JSON.stringify({ defaultProvider: 'magpie', defaultModel: 'benchmark-model' }));

const child = spawn(process.execPath, [fileURLToPath(new URL('../resources/pi-host/host.mjs', import.meta.url))], {
  env: { ...process.env, PI_CODING_AGENT_DIR: agentDir, FOLIO_USER_DATA: root,
    FOLIO_AI_SETTINGS: JSON.stringify({ baseUrl, titleModelId: '' }) },
  windowsHide: true,
});
const pending = new Map();
const sessions = new Map();
let requestId = 0;
let diagnostic = '';
child.stderr.on('data', data => { diagnostic += data; });
readline.createInterface({ input: child.stdout }).on('line', line => {
  const event = JSON.parse(line);
  const response = pending.get(event.requestId);
  if (event.type === 'response' && response) {
    pending.delete(event.requestId);
    response.resolve(event);
  }
  const measure = sessions.get(event.sessionId);
  if (measure && event.type === 'message_update') {
    measure.bytes += Buffer.byteLength(line) + 1;
    measure.events += 1;
    if (measure.firstTextMs === undefined
      && ['text_start', 'text_delta'].includes(event.assistantMessageEvent?.type)) {
      measure.firstTextMs = Number(process.hrtime.bigint() - measure.startedAt) / 1e6;
    }
  }
  if (event.type === 'agent_settled' && measure) {
    measure.settledMs = Number(process.hrtime.bigint() - measure.startedAt) / 1e6;
  }
});
child.on('exit', code => {
  for (const waiter of pending.values()) waiter.reject(new Error(`Pi host exited (${code}): ${diagnostic}`));
});

function command(fields) {
  const id = `bench-${++requestId}`;
  const result = Promise.withResolvers();
  pending.set(id, result);
  child.stdin.write(`${JSON.stringify({
    routeId: 'benchmark', requestId: id, cwd,
    projectKey: fs.realpathSync.native(cwd).replaceAll('/', '\\').replace(/\\+$/, '').toLowerCase(),
    ...fields,
  })}\n`);
  return result.promise;
}

try {
  const measurements = [];
  for (let round = 0; round < rounds; round += 1) {
    const created = await command({ type: 'create_session' });
    if (!created.success) throw new Error(created.error ?? diagnostic);
    const sessionId = created.data.sessionId;
    const measure = { startedAt: process.hrtime.bigint(), bytes: 0, events: 0 };
    sessions.set(sessionId, measure);
    const completed = await command({ type: 'prompt', sessionId, message: `benchmark ${round}` });
    if (!completed.success) throw new Error(completed.error ?? diagnostic);
    measurements.push({ bytes: measure.bytes, events: measure.events,
      firstTextMs: measure.firstTextMs, settledMs: measure.settledMs });
    sessions.delete(sessionId);
  }
  process.stdout.write(`${JSON.stringify({ rounds, chunkCount, chunkBytes, measurements })}\n`);
} finally {
  const exited = once(child, 'exit');
  child.stdin.end();
  child.kill();
  await exited;
  server.closeAllConnections();
  server.close();
  fs.rmSync(root, { recursive: true, force: true });
}
