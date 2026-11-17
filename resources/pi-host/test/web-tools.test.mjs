import test from 'node:test';
import assert from 'node:assert/strict';
import { webTools } from '../web-tools.mjs';

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
