/** Folio prepares an exact change set before committing tag edits. */
import { randomUUID } from 'node:crypto';
import { Type } from '@sinclair/typebox';
import { cancelled } from './bridge.mjs';

const text = Type.String({ minLength: 1 });
const paths = Type.Array(text, { minItems: 1, maxItems: 100,
  description: '仅接受具体文件的相对路径，不接受目录或通配符。传入目录会返回 file_not_found。文件夹提及请使用 under_directory。' });
const directory = Type.String({ minLength: 1,
  description: '当前项目或默认空间内的目录相对路径，包含所有子目录。「.」表示当前空间根目录。' });
const labels = Type.Array(text, { minItems: 1, maxItems: 20 });
const color = Type.Union(['gray', 'blue', 'green', 'orange', 'red', 'purple'].map(value => Type.Literal(value)));
const page = {
  limit: Type.Optional(Type.Integer({ minimum: 1, maximum: 100, description: '默认 50 条' })),
  cursor: Type.Optional(Type.String({ description: '使用上一次相同查询返回的 next_cursor' })),
};
const schema = fields => Type.Object(fields, { additionalProperties: false });
const resultForModel = (result, tagApproval) => ({
  content: [{ type: 'text', text: JSON.stringify(result) }],
  // Folio restores the same receipt from session history. The model only
  // receives the result, so the preview need not be repeated in its reply.
  details: tagApproval ? { ...result, tagApproval } : result,
  isError: result.status !== 'ok',
});

export function tagTools(request, confirm) {
  const read = operation => async (_id, params, signal) => resultForModel(await request(operation, params, signal));
  const write = operation => async (_toolCallId, params, signal) => {
    const proposal_id = randomUUID();
    try {
      const preview = await request(operation, { proposal_id, edit: params }, signal);
      if (preview.status === 'ok' && preview.delete_prompt) {
        const { title, message } = preview.delete_prompt;
        if (!await confirm(title, message, { signal })) return resultForModel(cancelled());
        return resultForModel(await request('commit', { proposal_id }, signal));
      }
      if (preview.status !== 'ok' || !preview.confirmation) return resultForModel(preview);
      return resultForModel(await request('commit', { proposal_id }, signal), preview.confirmation);
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
      description: '必须提供 paths、under_directory、name_contains、extension 或 tags 中至少一个查询条件；仅传 limit 会返回 invalid_request。查找当前项目或默认空间内的受支持文件及其标签。按文件夹递归查询请用 under_directory，列出整个空间可传 under_directory: "." 并分页。tags 默认要求全部匹配，match=any 匹配任一标签。结果只包含文件和标签信息。用 next_cursor 继续读取，避免在分页尚未结束时把结果当作全部。',
      parameters: schema({ paths: Type.Optional(paths), tags: Type.Optional(labels),
        match: Type.Optional(Type.Union([Type.Literal('all'), Type.Literal('any')])),
        under_directory: Type.Optional(directory),
        name_contains: Type.Optional(Type.String({ minLength: 1, description: '仅匹配文件名，不匹配目录名或完整路径。' })),
        extension: Type.Optional(text), ...page }), execute: read('query') },
    { name: 'folio_file_tags_update', label: '编辑 Folio 文件标签',
      description: '在 paths 和 under_directory 中二选一。paths 指定具体文件，under_directory 递归处理目录内所有受支持的文件。可在同一次调用里用 tag_colors 为 add 中的标签指定颜色。给新标签指定颜色时启用 create_missing。已有标签的颜色在当前空间全局生效。',
      parameters: schema({ paths: Type.Optional(paths), under_directory: Type.Optional(directory),
        add: Type.Optional(labels), remove: Type.Optional(labels),
        tag_colors: Type.Optional(Type.Array(schema({ tag: text, color }), {
          minItems: 1, maxItems: 20, description: '为 add 中的标签指定颜色，例如 [{ tag: "重点", color: "red" }]。',
        })),
        create_missing: Type.Optional(Type.Boolean()) }), execute: write('preview_files') },
    { name: 'folio_tags_manage', label: '管理 Folio 标签',
      description: '在当前项目或默认空间全局管理标签。rename 传 new_name，set_color 传 color。delete 只传 tag，用户确认后删除标签及其全部关联。更改少数文件的标签请调用 folio_file_tags_update。',
      parameters: schema({
        operation: Type.Union([Type.Literal('rename'), Type.Literal('set_color'), Type.Literal('delete')]), tag: text,
        new_name: Type.Optional(text),
        color: Type.Optional(color),
      }), execute: write('preview_definition') },
  ];
}
