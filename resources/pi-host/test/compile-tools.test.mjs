import { test } from 'node:test';
import assert from 'node:assert/strict';
import { compileTools } from '../compile-tools.mjs';
import { WorkspaceToolBridge } from '../workspace-tool-bridge.mjs';

test('compile replies stay with their originating session', async () => {
  const sent = [];
  const bridge = new WorkspaceToolBridge(event => sent.push(event), 'compile');
  const a = { projectKey: 'project-a', sessionId: 'a' };
  const b = { projectKey: 'project-b', sessionId: 'b' };
  const first = bridge.request(a, 'compile_tex', { paths: ['main.tex'] });
  const second = bridge.request(b, 'compile_tex', { under_directory: 'chapters', engine: 'lualatex' });
  assert.throws(() => bridge.respond({ ...b, id: sent[0].id, result: { status: 'ok' } }), /归属/);
  bridge.respond({ ...b, id: sent[1].id, result: { status: 'ok', summary: { total: 2, succeeded: 2 } } });
  bridge.respond({ ...a, id: sent[0].id, result: { status: 'ok', summary: { total: 1, succeeded: 1 } } });
  assert.deepEqual(await first, { status: 'ok', summary: { total: 1, succeeded: 1 } });
  assert.deepEqual(await second, { status: 'ok', summary: { total: 2, succeeded: 2 } });
});

test('cancelling a compile notifies Folio and retires its pending reply', async () => {
  const sent = [];
  const bridge = new WorkspaceToolBridge(event => sent.push(event), 'compile');
  const signal = new AbortController();
  const owner = { projectKey: 'project', sessionId: 'session' };
  const result = bridge.request(owner, 'compile_tex', { under_directory: '.' }, signal.signal);
  signal.abort();
  assert.deepEqual(sent[1], {
    ...owner, type: 'workspace_tool_cancel', channel: 'compile', id: sent[0].id,
  });
  bridge.respond({ ...owner, id: sent[0].id, result: { status: 'ok' } });
  assert.equal((await result).status, 'cancelled');
});

test('folio_tex_compile preserves structured batch errors without a diagnostic stack', async () => {
  const [tool] = compileTools(async () => ({
    status: 'error',
    error: { code: 'no_tex_targets', message: '目录 docs 下没有找到 .tex 文件。' },
  }));
  const result = await tool.execute('call', { under_directory: 'docs' });
  assert.equal(tool.name, 'folio_tex_compile');
  assert.equal(result.isError, true);
  assert.equal(JSON.parse(result.content[0].text).error.code, 'no_tex_targets');
});
