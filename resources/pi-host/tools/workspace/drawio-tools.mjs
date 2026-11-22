/** Read and write native Draw.io XML through the active Folio workspace. */
import { Type } from '@sinclair/typebox';

const path = Type.String({ minLength: 1, description: '当前 Folio 项目内的 .drawio 相对路径。' });
const xml = Type.String({ minLength: 1, description: '完整的、以 mxfile 为根元素的 Draw.io XML。' });
const parameters = fields => Type.Object(fields, { additionalProperties: false });
const cellId = Type.String({ minLength: 1, description: 'Draw.io cell ID。' });
const geometry = Type.Object({
  x: Type.Optional(Type.Number()),
  y: Type.Optional(Type.Number()),
  width: Type.Optional(Type.Number()),
  height: Type.Optional(Type.Number()),
  relative: Type.Optional(Type.Boolean()),
}, { additionalProperties: false });
const cellOperation = Type.Union([
  Type.Object({
    op: Type.Literal('add_vertex'),
    id: Type.Optional(cellId),
    value: Type.String(),
    x: Type.Number(),
    y: Type.Number(),
    width: Type.Number(),
    height: Type.Number(),
    style: Type.Optional(Type.String()),
    parent_id: Type.Optional(cellId),
  }, { additionalProperties: false }),
  Type.Object({
    op: Type.Literal('add_edge'),
    id: Type.Optional(cellId),
    value: Type.String(),
    source_id: cellId,
    target_id: cellId,
    style: Type.Optional(Type.String()),
    parent_id: Type.Optional(cellId),
  }, { additionalProperties: false }),
  Type.Object({
    op: Type.Literal('update'),
    id: cellId,
    value: Type.Optional(Type.String()),
    style: Type.Optional(Type.String()),
    geometry: Type.Optional(geometry),
    source_id: Type.Optional(cellId),
    target_id: Type.Optional(cellId),
  }, { additionalProperties: false }),
  Type.Object({ op: Type.Literal('delete'), id: cellId }, { additionalProperties: false }),
]);

function toolResult(result) {
  return {
    content: [{ type: 'text', text: JSON.stringify(result) }],
    details: result,
    isError: result.status !== 'ok',
  };
}

export function drawioTools(request) {
  const read = {
    name: 'folio_drawio_read',
    label: '读取 Draw.io 图表',
    description: '读取当前 Folio 项目中的 .drawio 源文件。修改已有图表前先读取目标文件，再将完整 XML 传给 folio_drawio_update。',
    parameters: parameters({ path }),
    execute: async (_id, params, signal) => toolResult(await request('read', params, signal)),
  };
  const create = {
    name: 'folio_drawio_create',
    label: '新建 Draw.io 图表',
    description: '在当前 Folio 项目中创建 .drawio 文件。xml 必须是完整、可解析且以 mxfile 为根元素的 Draw.io XML。目标已存在时不会覆盖。',
    parameters: parameters({ path, xml }),
    execute: async (_id, params, signal) => toolResult(await request('create', params, signal)),
  };
  const update = {
    name: 'folio_drawio_update',
    label: '更新 Draw.io 图表',
    description: '用完整的 Draw.io XML 替换当前 Folio 项目中已有的 .drawio 文件。修改前先用 folio_drawio_read 读取原图。',
    parameters: parameters({ path, xml }),
    execute: async (_id, params, signal) => toolResult(await request('update', params, signal)),
  };
  const getSelection = {
    name: 'folio_drawio_get_selection',
    label: '读取 Draw.io 选区',
    description: '读取当前打开的 Draw.io 画布中用户选中的元素。返回 cell ID、文本、样式、几何位置、父级和连线端点。',
    parameters: parameters({}),
    execute: async (_id, params, signal) => toolResult(await request('get_selection', params, signal)),
  };
  const editCells = {
    name: 'folio_drawio_edit_cells',
    label: '编辑 Draw.io 元素',
    description: '按 cell ID 在当前打开的 Draw.io 画布中批量新增、更新或删除元素。新增顶点需提供 x、y、width、height。新增连线需提供 source_id 和 target_id。更新可修改 value、style、geometry 或连线端点。',
    parameters: parameters({ operations: Type.Array(cellOperation, { minItems: 1 }) }),
    execute: async (_id, params, signal) => toolResult(await request('edit_cells', params, signal)),
  };

  return [read, create, update, getSelection, editCells];
}
