import { test } from 'node:test';
import assert from 'node:assert/strict';
import { Value } from '@sinclair/typebox/value';
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
  const addNodeSchema = applyTool.parameters.properties.ops.items.anyOf.find(
    s => s.properties?.op?.const === 'add_node',
  );
  assert.ok(addNodeSchema, 'add_node op schema should exist');
  assert.ok(addNodeSchema.properties.file, 'add_node should accept file path');
  const nodeTypes = addNodeSchema.properties.node_type.anyOf.map(s => s.const);
  assert.deepEqual(nodeTypes, ['text', 'file', 'group']);

  const res = await applyTool.execute('id-1', {
    path: 'maps/test.canvas',
    expected_revision: 'rev1',
    ops: [{ op: 'add_node', node_type: 'file', file: '../assets/diagram.png', text: 'Diagram' }],
  });
  assert.equal(res.isError, false);
  assert.equal(calls[0].operation, 'apply_ops');
  assert.equal(calls[0].params.path, 'maps/test.canvas');
  assert.equal(calls[0].params.ops[0].file, '../assets/diagram.png');

  const createTool = tools.find(t => t.name === 'folio_canvas_create');
  await createTool.execute('id-2', {
    path: 'maps/tree.canvas',
    view_mode: 'tree',
    tree_mode: 'tree_down',
  });
  assert.equal(calls[1].operation, 'create');
  assert.equal(calls[1].params.view_mode, 'tree');
  assert.equal(calls[1].params.tree_mode, 'tree_down');

  const layoutTool = tools.find(t => t.name === 'folio_canvas_layout');
  const layoutParams = {
    path: 'maps/tree.canvas',
    expected_revision: 'rev2',
    view_mode: 'tree',
    mode: 'mindmap_right',
    root_id: 'root',
  };
  assert.ok(Value.Check(layoutTool.parameters, layoutParams));
  await layoutTool.execute('id-3', layoutParams);
  assert.equal(calls[2].operation, 'layout');
  assert.equal(calls[2].params.view_mode, 'tree');
  assert.equal(calls[2].params.expected_revision, 'rev2');
  assert.equal(calls[2].params.mode, 'mindmap_right');
  assert.equal(calls[2].params.root_id, 'root');
});
