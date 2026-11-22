import { test } from 'node:test';
import assert from 'node:assert/strict';
import { tagTools } from '../tools/workspace/tag-tools.mjs';

const confirmation = {
  kind:'files', created_tags:[], files:[{path:'note.md', add:['重点'], remove:[]}],
};

test('tag updates commit the preview directly and discard the proposal receipt', async t => {
  const request = t.mock.fn(async () => ({status:'ok', updated:1}));
  request.mock.mockImplementationOnce(async () => ({status:'ok', confirmation}));
  const tool = tagTools(request).find(tool => tool.name === 'folio_file_tags_update');

  const result = await tool.execute('call', {paths:['note.md'], add:['重点']});

  assert.equal(result.details.status, 'ok');
  assert.deepEqual(request.mock.calls.map(call => call.arguments[0]), ['preview_files', 'commit', 'discard']);
  assert.equal(request.mock.calls[0].arguments[1].proposal_id, request.mock.calls[1].arguments[1].proposal_id);
  assert.equal(request.mock.calls[1].arguments[1].proposal_id, request.mock.calls[2].arguments[1].proposal_id);
});
