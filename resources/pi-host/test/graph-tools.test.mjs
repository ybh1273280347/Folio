import { test } from 'node:test';
import assert from 'node:assert/strict';
import { graphTools } from '../tools/workspace/graph-tools.mjs';

test('graph tools forward operation names and mark ok/error responses', async t => {
  const request = t.mock.fn(async (op, params) => {
    if (op === 'neighbors') {
      return { status: 'ok', center: { path: params.path }, inbound: [], outbound: [], two_hop: [] };
    }
    if (op === 'overview') {
      return { status: 'ok', summary: { markdown_count: 3 }, hubs: [], orphans: [], unresolved: [] };
    }
    return { status: 'error', error: { code: 'node_not_found', message: 'missing', retryable: false } };
  });

  const tools = graphTools(request);
  assert.deepEqual(
    tools.map(tool => tool.name),
    ['folio_graph_neighbors', 'folio_graph_overview', 'folio_graph_path'],
  );

  const neighbors = await tools[0].execute('c1', { path: 'a.md', depth: 2 });
  assert.equal(neighbors.isError, false);
  assert.equal(request.mock.calls[0].arguments[0], 'neighbors');

  const overview = await tools[1].execute('c2', { limit: 10 });
  assert.equal(overview.isError, false);
  assert.equal(request.mock.calls[1].arguments[0], 'overview');

  const pathRes = await tools[2].execute('c3', { from_path: 'a.md', to_path: 'b.md' });
  assert.equal(pathRes.isError, true);
  assert.equal(request.mock.calls[2].arguments[0], 'path');
});
