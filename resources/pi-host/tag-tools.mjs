/** Folio prepares an exact change set before the SDK asks its user for approval. */
import { randomUUID } from 'node:crypto';
import { Type } from '@sinclair/typebox';
import { cancelled } from './workspace-tool-bridge.mjs';

const text = Type.String({ minLength: 1 });
const paths = Type.Array(text, { minItems: 1, maxItems: 100,
  description: '当前项目或默认空间内的具体相对路径。使用检索返回的路径，不使用通配符。' });
const labels = Type.Array(text, { minItems: 1, maxItems: 20 });
const page = {
  limit: Type.Optional(Type.Integer({ minimum: 1, maximum: 100, description: '默认 50 条' })),
  cursor: Type.Optional(Type.String({ description: '使用上一次相同查询返回的 next_cursor' })),
};
const schema = fields => Type.Object(fields, { additionalProperties: false });
const resultForModel = result => ({
  content: [{ type: 'text', text: JSON.stringify(result) }],
  details: result,
  isError: result.status !== 'ok',
});

export function tagTools(request, confirm) {
  const read = operation => async (_id, params, signal) => resultForModel(await request(operation, params, signal));
  const write = operation => async (_id, params, signal) => {
    const proposal_id = randomUUID();
    try {
      const preview = await request(operation, { proposal_id, edit: params }, signal);
      if (preview.status !== 'ok' || !preview.confirmation) return resultForModel(preview);
      const allowed = await confirm('修改 Folio 标签', preview.confirmation, { signal });
      if (signal?.aborted) return resultForModel(cancelled());
      if (!allowed) return resultForModel({
        status: 'declined', error: { code: 'declined', message: '用户拒绝了这批标签修改。' },
      });
      return resultForModel(await request('commit', { proposal_id }, signal));
    } finally {
      // This also clears a proposal if cancellation raced its preview reply.
      await request('discard', { proposal_id });
    }
  };

  return [
    { name: 'folio_tags_list', label: '查看 Folio 标签',
      description: '列出当前项目或默认空间中的标签名称和颜色。按名称查找现有标签时先用此工具。独立文件空间不开放。',
      parameters: schema({ query: Type.Optional(text), ...page }), execute: read('list') },
    { name: 'folio_files_query', label: '按标签查找文件',
      description: '查找当前项目或默认空间的受支持文件及其标签。tags 默认要求全部匹配，match=any 匹配任一标签。支持按文件名、扩展名或指定路径查询。结果只包含文件和标签信息。用 next_cursor 继续读取，避免在分页尚未结束时把结果当作全部。独立文件空间不开放。',
      parameters: schema({ paths: Type.Optional(paths), tags: Type.Optional(labels),
        match: Type.Optional(Type.Union([Type.Literal('all'), Type.Literal('any')])),
        name_contains: Type.Optional(text), extension: Type.Optional(text), ...page }), execute: read('query') },
    { name: 'folio_file_tags_update', label: '编辑 Folio 文件标签',
      description: '为具体文件添加和移除标签，保留其它标签。需要创建新标签时才设 create_missing。Folio 会把整批具体变化展示给用户确认。用户拒绝或取消后停止该修改。只有提交返回成功时才能报告已更改。',
      parameters: schema({ paths, add: Type.Optional(labels), remove: Type.Optional(labels),
        create_missing: Type.Optional(Type.Boolean()) }), execute: write('preview_files') },
    { name: 'folio_tags_manage', label: '管理 Folio 标签',
      description: '全局重命名标签或更改标签颜色，影响当前项目或默认空间内所有引用。rename 传 new_name，set_color 传 color。更改少数文件的标签请调用 folio_file_tags_update。',
      parameters: schema({
        operation: Type.Union([Type.Literal('rename'), Type.Literal('set_color')]), tag: text,
        new_name: Type.Optional(text),
        color: Type.Optional(Type.Union(['gray', 'blue', 'green', 'orange', 'red', 'purple'].map(value => Type.Literal(value)))),
      }), execute: write('preview_definition') },
  ];
}
