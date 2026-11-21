import { test } from 'node:test';
import assert from 'node:assert/strict';
import { drawioTools } from '../drawio-tools.mjs';

test('Draw.io tools expose read, create, and update operations through the workspace bridge', async () => {
  const calls = [];
  const tools = drawioTools(async (operation, params) => {
    calls.push({ operation, params });
    return { status: 'ok', path: params.path };
  });

  await tools[0].execute('read', { path: 'architecture.drawio' });
  await tools[1].execute('create', { path: 'new.drawio', xml: '<mxfile/>' });
  await tools[2].execute('update', { path: 'architecture.drawio', xml: '<mxfile/>' });

  assert.deepEqual(calls, [
    { operation: 'read', params: { path: 'architecture.drawio' } },
    { operation: 'create', params: { path: 'new.drawio', xml: '<mxfile/>' } },
    { operation: 'update', params: { path: 'architecture.drawio', xml: '<mxfile/>' } },
  ]);
  assert.deepEqual(tools.map(tool => tool.name), [
    'folio_drawio_read', 'folio_drawio_create', 'folio_drawio_update',
  ]);
});

test('Draw.io tools preserve structured workspace errors', async () => {
  const tools = drawioTools(async () => ({
    status: 'error',
    error: { code: 'file_exists', message: '目标文件已存在。' },
  }));

  const result = await tools[1].execute('create', { path: 'existing.drawio', xml: '<mxfile/>' });

  assert.equal(result.isError, true);
  assert.equal(JSON.parse(result.content[0].text).error.code, 'file_exists');
});
