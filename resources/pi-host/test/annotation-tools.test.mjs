import { test } from 'node:test';
import assert from 'node:assert/strict';
import { annotationTools } from '../tools/workspace/annotation-tools.mjs';
import { WorkspaceToolBridge } from '../tools/workspace/bridge.mjs';

test('annotation replies stay with their originating session', async () => {
  const sent = [];
  const bridge = new WorkspaceToolBridge(event => sent.push(event), 'annotation');
  const a = { projectKey:'project-a', sessionId:'a' };
  const b = { projectKey:'project-b', sessionId:'b' };
  const first = bridge.request(a, 'create', { path:'a.md', quote:'first', comment:'note' });
  const second = bridge.request(b, 'list', { path:'b.pdf' });
  assert.throws(() => bridge.respond({ ...b, id:sent[0].id, result:{status:'ok'} }), /归属/);
  bridge.respond({ ...b, id:sent[1].id, result:{status:'ok', annotations:[]} });
  bridge.respond({ ...a, id:sent[0].id, result:{status:'ok', annotation:{id:7}} });
  assert.deepEqual(await first, {status:'ok', annotation:{id:7}});
  assert.deepEqual(await second, {status:'ok', annotations:[]});
});

test('cancelling an annotation notifies Folio and retires its pending reply', async () => {
  const sent = [];
  const bridge = new WorkspaceToolBridge(event => sent.push(event), 'annotation');
  const signal = new AbortController();
  const owner = {projectKey:'project', sessionId:'session'};
  const result = bridge.request(owner, 'create', {path:'note.md'}, signal.signal);
  signal.abort();
  assert.deepEqual(sent[1], {
    ...owner, type: 'workspace_tool_cancel', channel: 'annotation', id: sent[0].id,
  });
  bridge.respond({...owner, id:sent[0].id, result:{status:'ok'}});
  assert.equal((await result).status, 'cancelled');
});

test('tools preserve structured placement errors without a diagnostic stack', async () => {
  const tool = annotationTools(async () => ({status:'error', error:{code:'ambiguous_match', message:'choose a match'}, candidates:[{occurrence:1}]}))[1];
  const result = await tool.execute('call', {path:'note.md', quote:'same', comment:'note'});
  assert.equal(tool.name, 'folio_annotation_create');
  assert.equal(result.isError, true);
  assert.equal(JSON.parse(result.content[0].text).error.code, 'ambiguous_match');
  assert.equal(result.details.candidates[0].occurrence, 1);
});

test('annotation edits address a file-local ID and keep empty comments', async () => {
  const requests = [];
  const tools = annotationTools(async (operation, params) => {
    requests.push({ operation, params });
    return { status: 'ok' };
  });
  const update = tools.find(tool => tool.name === 'folio_annotation_update');
  const remove = tools.find(tool => tool.name === 'folio_annotation_delete');

  await update.execute('update', { path: 'notes.md', id: 7, comment: '' });
  await remove.execute('delete', { path: 'notes.md', id: 7 });

  assert.deepEqual(requests, [
    { operation: 'update', params: { path: 'notes.md', id: 7, comment: '' } },
    { operation: 'delete', params: { path: 'notes.md', id: 7 } },
  ]);
});
