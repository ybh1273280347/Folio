import { test } from 'node:test';
import assert from 'node:assert/strict';
import { tagTools } from '../tag-tools.mjs';

const confirmation = {
  kind:'files', created_tags:[], files:[{path:'note.md', add:['重点'], remove:[]}],
};

test('refusing the tag confirmation discards the proposal without a commit', async t => {
  const request = t.mock.fn(async () => ({status:'ok'}));
  request.mock.mockImplementationOnce(async () => ({status:'ok', confirmation}));
  const confirm = t.mock.fn(async () => false);
  const tool = tagTools(request, confirm).find(tool => tool.name === 'folio_file_tags_update');

  const result = await tool.execute('call', {paths:['note.md'], add:['重点']});

  assert.equal(result.details.status, 'declined');
  assert.deepEqual(request.mock.calls.map(call => call.arguments[0]), ['preview_files', 'discard']);
  assert.equal(request.mock.calls[0].arguments[1].proposal_id, request.mock.calls[1].arguments[1].proposal_id);
});

test('stopping during tag confirmation discards the proposal and reports cancellation', async t => {
  const stop = new AbortController();
  const request = t.mock.fn(async () => ({status:'ok'}));
  request.mock.mockImplementationOnce(async () => ({status:'ok', confirmation}));
  const tool = tagTools(request, async () => { stop.abort(); return false; })
    .find(tool => tool.name === 'folio_file_tags_update');

  const result = await tool.execute('call', {paths:['note.md'], add:['重点']}, stop.signal);

  assert.equal(result.details.status, 'cancelled');
  assert.deepEqual(request.mock.calls.map(call => call.arguments[0]), ['preview_files', 'discard']);
});
