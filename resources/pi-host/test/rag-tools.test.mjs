import { test } from 'node:test';
import assert from 'node:assert/strict';
import { ragTools } from '../tools/workspace/rag-tools.mjs';

test('folio_kb_search forwards typed searches and intent directly without double-wrapping params', async t => {
  const request = t.mock.fn(async () => ({
    status: 'ok',
    modeUsed: 'hybrid+rerank',
    indexedFiles: 12,
    pendingVectors: 0,
    results: [{ docid: 'abcd1234', path: 'notes/opt.md', lineStart: 10, lineEnd: 24, score: 0.08, snippet: 'KKT' }],
  }));
  const tool = ragTools(request).find(item => item.name === 'folio_kb_search');
  const signal = new AbortController().signal;

  const result = await tool.execute('call-1', {
    searches: [
      { type: 'lex', query: 'KKT complementary slackness' },
      { type: 'vec', query: '互补松弛条件什么时候成立' },
    ],
    intent: '互补松弛条件',
    limit: 5,
  }, signal);

  assert.equal(result.isError, false);
  assert.equal(result.details.modeUsed, 'hybrid+rerank');
  assert.equal(request.mock.calls[0].arguments[0], 'search');
  assert.deepEqual(request.mock.calls[0].arguments[1], {
    searches: [
      { type: 'lex', query: 'KKT complementary slackness' },
      { type: 'vec', query: '互补松弛条件什么时候成立' },
    ],
    intent: '互补松弛条件',
    limit: 5,
  });
  assert.equal(request.mock.calls[0].arguments[2], signal);
});

test('folio_kb_status and folio_kb_reindex forward accurate options and map cancellation', async t => {
  const request = t.mock.fn(async op => (op === 'status'
    ? { status: 'ok', report: { indexedFiles: 3 } }
    : { status: 'cancelled', error: { code: 'cancelled', message: 'cancelled' } }));

  const tools = ragTools(request);
  const statusTool = tools.find(item => item.name === 'folio_kb_status');
  const reindexTool = tools.find(item => item.name === 'folio_kb_reindex');

  const statusRes = await statusTool.execute('call-2', {});
  assert.equal(statusRes.isError, false);
  assert.deepEqual(request.mock.calls[0].arguments.slice(0, 2), ['status', {}]);

  const reindexRes = await reindexTool.execute('call-3', {
    force_rebuild: true,
    accurate: 'only_ocr',
    paths: ['papers/scan.pdf'],
  });
  assert.equal(reindexRes.isError, true);
  assert.deepEqual(
    request.mock.calls[1].arguments.slice(0, 2),
    ['reindex', { force_rebuild: true, accurate: 'only_ocr', paths: ['papers/scan.pdf'] }],
  );
});
