import { test } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';

test('the Draw.io App integration reads selection and applies ID-based edits atomically', () => {
  const appPath = fileURLToPath(new URL('../../drawio/js/diagramly/App.js', import.meta.url));
  const appSource = fs.readFileSync(appPath, 'utf8');
  const methodStart = appSource.indexOf('App.prototype.installFolioDrawioTools = function()');
  const methodEnd = appSource.indexOf('\n};\n\n/**\n * Initializes the UI', methodStart) + 3;
  const installMethod = appSource.slice(methodStart, methodEnd);
  const first = {
    id: 'node-1', value: 'User', style: 'rounded=1', vertex: true,
    geometry: { x: 10, y: 20, width: 80, height: 40, relative: false, clone() { return { ...this }; } },
    parent: { id: 'layer-1' },
  };
  const second = {
    id: 'node-2', value: 'Database', style: '', vertex: true,
    geometry: { x: 200, y: 20, width: 90, height: 40, relative: false, clone() { return { ...this }; } },
    parent: { id: 'layer-1' },
  };
  const cells = new Map([[first.id, first], [second.id, second]]);
  const transactions = { started: 0, ended: 0 };
  const model = {
    getCell: id => cells.get(id),
    getGeometry: cell => cell.geometry,
    isEdge: cell => cell.edge === true,
    isVertex: cell => cell.vertex === true,
    beginUpdate: () => { transactions.started += 1; },
    endUpdate: () => { transactions.ended += 1; },
    setValue: (cell, value) => { cell.value = value; },
    setStyle: (cell, style) => { cell.style = style; },
    setGeometry: (cell, geometry) => { cell.geometry = geometry; },
    setTerminal: (cell, terminal, source) => { cell[source ? 'source' : 'target'] = terminal; },
  };
  const layer = { id: 'layer-1' };
  const graph = {
    getModel: () => model,
    getDefaultParent: () => layer,
    getSelectionCells: () => [first],
    insertVertex: (parent, id, value, x, y, width, height, style) => cells.set(id, {
      id, value, style, vertex: true, parent,
      geometry: { x, y, width, height, relative: false, clone() { return { ...this }; } },
    }),
    insertEdge: (parent, id, value, source, target, style) => cells.set(id, {
      id, value, style, edge: true, parent, source, target,
      geometry: { relative: true, clone() { return { ...this }; } },
    }),
    removeCells: removed => removed.forEach(cell => cells.delete(cell.id)),
  };
  const results = [];
  const window = { ipc: { postMessage: message => results.push(JSON.parse(message)) } };
  function App() {}
  vm.runInNewContext(installMethod, { App, window, Date, Map, Set, Object, String, Error });
  App.prototype.installFolioDrawioTools.call({ editor: { graph } });

  window.__folioDrawioTool({
    request_id: 'selection', operation: 'get_selection', params: {},
  });
  assert.deepEqual(JSON.parse(JSON.stringify(results[0].result.cells[0])), {
    id: 'node-1', kind: 'vertex', value: 'User', style: 'rounded=1',
    geometry: { x: 10, y: 20, width: 80, height: 40, relative: false },
    parent_id: 'layer-1', source_id: null, target_id: null,
  });

  window.__folioDrawioTool({
    request_id: 'edit', operation: 'edit_cells', params: { operations: [
      { op: 'update', id: 'node-1', value: 'Authentication', geometry: { x: 40 } },
      { op: 'add_vertex', id: 'token-store', value: 'Token store', x: 300, y: 20, width: 100, height: 40 },
      { op: 'add_edge', id: 'auth-token', value: 'writes', source_id: 'node-1', target_id: 'token-store' },
      { op: 'delete', id: 'node-2' },
    ] },
  });
  assert.equal(results[1].result.status, 'ok', JSON.stringify(results[1].result));
  assert.equal(results[1].result.applied, 4);
  assert.equal(model.getCell('node-1').value, 'Authentication');
  assert.equal(model.getCell('node-1').geometry.x, 40);
  assert.equal(model.getCell('auth-token').target.id, 'token-store');
  assert.equal(model.getCell('node-2'), undefined);
  assert.equal(transactions.started, 1);
  assert.equal(transactions.ended, 1);

  window.__folioDrawioTool({
    request_id: 'invalid-edit', operation: 'edit_cells', params: { operations: [
      { op: 'update', id: 'node-1', value: 'Should not apply' },
      { op: 'add_edge', id: 'bad-edge', value: '', source_id: 'missing', target_id: 'token-store' },
    ] },
  });
  assert.equal(results[2].result.status, 'error');
  assert.equal(model.getCell('node-1').value, 'Authentication');
  assert.equal(transactions.started, 1);
});
