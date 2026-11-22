import { test } from 'node:test';
import assert from 'node:assert/strict';
import { ragTools } from '../tools/workspace/rag-tools.mjs';

test('folio_kb_search forwards hybrid query params and returns structured hits', async t => {
  const request = t.mock.fn(async () => ({
    status: 'ok',
    modeUsed: 'hybrid',
    indexedFiles: 12,
    pendingVectors: 0,
    results: [{ docid: 'abcd1234', path: 'notes/opt.md', lineStart: 10, lineEnd: 24, score: 0.08, snippet: 'KKT' }],
  }));
  const tool = ragTools(request).find(item => item.name === 'folio_kb_search');
  const signal = new AbortController().signal;

  const result = await tool.execute('call-1', {
    query: '互补松弛条件',
    lex_queries: ['KKT', 'complementary slackness'],
    mode: 'hybrid',
    limit: 5,
  }, signal);

  assert.equal(result.isError, false);
  assert.equal(result.details.modeUsed, 'hybrid');
  assert.equal(request.mock.calls[0].arguments[0], 'search');
  assert.deepEqual(request.mock.calls[0].arguments[1], {
    params: {
      query: '互补松弛条件',
      lex_queries: ['KKT', 'complementary slackness'],
      mode: 'hybrid',
      limit: 5,
    },
  });
  assert.equal(request.mock.calls[0].arguments[2], signal);
});

test('folio_kb_status and folio_kb_reindex map errors and cancellation to isError', async t => {
  const request = t.mock.fn(async op => (op === 'status'
    ? { status: 'ok', report: { indexedFiles: 3 } }
    : { status: 'cancelled', error: { code: 'cancelled', message: 'cancelled' } }));

  const tools = ragTools(request);
  const statusTool = tools.find(item => item.name === 'folio_kb_status');
  const reindexTool = tools.find(item => item.name === 'folio_kb_reindex');

  const statusRes = await statusTool.execute('call-2', {});
  assert.equal(statusRes.isError, false);
  assert.deepEqual(request.mock.calls[0].arguments.slice(0, 2), ['status', {}]);

  const reindexRes = await reindexTool.execute('call-3', { force_rebuild: true });
  assert.equal(reindexRes.isError, true);
  assert.deepEqual(request.mock.calls[1].arguments.slice(0, 2), ['reindex', { params: { force_rebuild: true } }]);
});
