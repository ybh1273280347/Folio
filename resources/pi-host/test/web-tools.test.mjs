import test from 'node:test';
import assert from 'node:assert/strict';
import childProcess from 'node:child_process';
import http from 'node:http';
import { once } from 'node:events';
import { webTools } from '../tools/web/tools.mjs';

test.beforeEach(t => {
  t.mock.method(childProcess, 'execFile', (_command, _args, _options, callback) => {
    callback(null, 'http://127.0.0.1:7897/\r\n');
  });
});

test('Exa reaches its destination through the system proxy instead of a direct connection', { timeout: 10000 }, async t => {
  const connections = [];
  const proxy = http.createServer();
  proxy.on('connect', (request, socket) => {
    connections.push(request.url);
    socket.end('HTTP/1.1 502 Bad Gateway\r\nConnection: close\r\nContent-Length: 0\r\n\r\n');
  });
  proxy.listen(0, '127.0.0.1');
  await once(proxy, 'listening');
  t.after(() => proxy.close());
  childProcess.execFile.mock.mockImplementation((_command, _args, options, callback) => {
    assert.deepEqual(Object.keys(options.env).filter(name => /^(https?|all|no)_proxy$/i.test(name)), []);
    callback(null, `http://127.0.0.1:${proxy.address().port}/`);
  });
  const result = await webTools(() => ({ exaApiKey: 'key' }))[0].execute('call', { query: '测试' });
  assert.deepEqual(connections, ['api.exa.ai:443']);
  assert.equal(result.isError, true);
  assert.equal(result.details.error.type, 'connection_error');
});

test('Exa reports missing system proxy configuration without trying a direct request', async t => {
  childProcess.execFile.mock.mockImplementation((_command, _args, _options, callback) => {
    callback(null, 'https://api.exa.ai/search');
  });
  const network = t.mock.method(globalThis, 'fetch', async () => Response.json({ results: [] }));
  const result = await webTools(() => ({ exaApiKey: 'key' }))[0].execute('call', { query: '测试' });
  assert.equal(result.details.error.type, 'configuration_error');
  assert.equal(result.isError, true);
  assert.equal(network.mock.callCount(), 0);
});

test('connection errors are concise failures rather than empty search results or stack traces', async t => {
  const network = t.mock.method(globalThis, 'fetch', async () => {
    throw new TypeError('fetch failed', { cause: new Error('private transport diagnostics') });
  });
  const tool = webTools(() => ({ exaApiKey: 'private-key' }))[0];
  const result = await tool.execute('call', { query: '测试' }, new AbortController().signal);
  assert.equal(result.isError, true);
  assert.deepEqual(result.details, { status: 'error', error: {
    type: 'connection_error', message: '无法连接搜索服务，请检查网络、代理或 DNS。', retryable: true,
  } });
  assert.doesNotMatch(result.content[0].text, /private|TypeError|\n\s+at /);
  assert.equal(network.mock.callCount(), 1);
});

test('rate limits retain their HTTP status and retry guidance without returning the remote body', async t => {
  t.mock.method(globalThis, 'fetch', async () => new Response('private remote diagnostics', { status: 429 }));
  const result = await webTools(() => ({ exaApiKey: 'key' }))[0].execute('call', { query: '测试' });
  assert.equal(result.isError, true);
  assert.equal(result.details.error.type, 'rate_limited');
  assert.equal(result.details.error.http_status, 429);
  assert.equal(result.details.error.retryable, true);
  assert.doesNotMatch(result.content[0].text, /private/);
});

test('empty search results remain successful requests with an explicit empty status', async t => {
  t.mock.method(globalThis, 'fetch', async () => Response.json({ results: [] }));
  const result = await webTools(() => ({ exaApiKey: 'key' }))[0].execute('call', { query: '测试' });
  assert.equal(result.isError, false);
  assert.equal(result.details.status, 'empty');
  assert.deepEqual(result.details.candidates, []);
  assert.equal(result.details.error, undefined);
});

test('malformed provider responses are distinguished from searches without matches', async t => {
  t.mock.method(globalThis, 'fetch', async () => new Response('<html>gateway error</html>'));
  const result = await webTools(() => ({ baiduQianfanApiKey: 'key' }))[1].execute('call', { query: '测试' });
  assert.equal(result.isError, true);
  assert.equal(result.details.status, 'error');
  assert.equal(result.details.error.type, 'invalid_response');
  assert.doesNotMatch(result.content[0].text, /SyntaxError|gateway error/);
});

test('cancellation is explicitly reported and never starts a request', async t => {
  const network = t.mock.method(globalThis, 'fetch', async () => Response.json({ results: [] }));
  const controller = new AbortController();
  controller.abort(new Error('private cancellation reason'));
  const result = await webTools(() => ({ exaApiKey: 'key' }))[0].execute('call', { query: '测试' }, controller.signal);
  assert.equal(result.details.status, 'cancelled');
  assert.equal(result.details.error.type, 'cancelled');
  assert.equal(result.isError, true);
  assert.equal(network.mock.callCount(), 0);
  assert.doesNotMatch(result.content[0].text, /private/);
});
