import { test } from 'node:test';
import assert from 'node:assert/strict';
import { canvasTools } from '../tools/workspace/canvas-tools.mjs';

test('canvasTools exposes expected tool names and forwards workspace requests', async () => {
  const calls = [];
  const request = async (operation, params) => {
    calls.push({ operation, params });
    return { status: 'ok', operation };
  };

  const tools = canvasTools(request);
  assert.deepEqual(
    tools.map(t => t.name),
    [
      'folio_canvas_read',
      'folio_canvas_get_selection',
      'folio_canvas_create',
      'folio_canvas_apply_ops',
      'folio_canvas_layout',
    ],
  );

  const applyTool = tools.find(t => t.name === 'folio_canvas_apply_ops');
  const res = await applyTool.execute('id-1', {
    path: 'maps/test.canvas',
    expected_revision: 'rev1',
    ops: [{ op: 'add_node', text: 'Concept A' }],
  });
  assert.equal(res.isError, false);
  assert.equal(calls[0].operation, 'apply_ops');
  assert.equal(calls[0].params.path, 'maps/test.canvas');
});
