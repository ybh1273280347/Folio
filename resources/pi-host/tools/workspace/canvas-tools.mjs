/** Structured read, incremental edit, and layout tools for Folio `.canvas` files. */
import { Type } from '@sinclair/typebox';

const path = Type.String({ minLength: 1, description: '当前 Folio 项目内的 .canvas 相对路径。' });
const nodeId = Type.String({ minLength: 1, description: '画布节点 ID。' });
const edgeId = Type.String({ minLength: 1, description: '画布连线 ID。' });
const parameters = fields => Type.Object(fields, { additionalProperties: false });

const anchorSchema = Type.Union([
  Type.Object({
    kind: Type.Literal('markdown_file'),
    path: Type.String({ minLength: 1 }),
  }, { additionalProperties: false }),
  Type.Object({
    kind: Type.Literal('markdown_heading'),
    path: Type.String({ minLength: 1 }),
    heading: Type.String({ minLength: 1 }),
    heading_path: Type.Optional(Type.Array(Type.String())),
    line_hint: Type.Optional(Type.Integer({ minimum: 1 })),
  }, { additionalProperties: false }),
  Type.Object({
    kind: Type.Literal('markdown_quote'),
    path: Type.String({ minLength: 1 }),
    quote: Type.String({ minLength: 1 }),
    occurrence: Type.Optional(Type.Integer({ minimum: 0 })),
    heading_path: Type.Optional(Type.Array(Type.String())),
    line_hint: Type.Optional(Type.Integer({ minimum: 1 })),
  }, { additionalProperties: false }),
  Type.Object({
    kind: Type.Literal('pdf_page'),
    path: Type.String({ minLength: 1 }),
    page: Type.Integer({ minimum: 1 }),
    quote: Type.Optional(Type.String()),
  }, { additionalProperties: false }),
  Type.Object({
    kind: Type.Literal('kb_chunk'),
    path: Type.String({ minLength: 1 }),
    docid: Type.Optional(Type.String()),
    heading: Type.Optional(Type.String()),
    line_start: Type.Optional(Type.Integer({ minimum: 1 })),
    page: Type.Optional(Type.Integer({ minimum: 1 })),
    snippet: Type.Optional(Type.String()),
  }, { additionalProperties: false }),
]);

const opSchema = Type.Union([
  Type.Object({
    op: Type.Literal('add_node'),
    id: Type.Optional(nodeId),
    node_type: Type.Optional(Type.Union(['text', 'file', 'link', 'group'].map(v => Type.Literal(v)))),
    text: Type.Optional(Type.String()),
    x: Type.Optional(Type.Number()),
    y: Type.Optional(Type.Number()),
    width: Type.Optional(Type.Number()),
    height: Type.Optional(Type.Number()),
    color: Type.Optional(Type.String()),
    parent_id: Type.Optional(nodeId),
    pinned: Type.Optional(Type.Boolean()),
    anchor: Type.Optional(anchorSchema),
  }, { additionalProperties: false }),
  Type.Object({
    op: Type.Literal('update_node'),
    id: nodeId,
    text: Type.Optional(Type.String()),
    x: Type.Optional(Type.Number()),
    y: Type.Optional(Type.Number()),
    width: Type.Optional(Type.Number()),
    height: Type.Optional(Type.Number()),
    color: Type.Optional(Type.String()),
    pinned: Type.Optional(Type.Boolean()),
    collapsed: Type.Optional(Type.Boolean()),
    anchor: Type.Optional(Type.Union([anchorSchema, Type.Null()])),
  }, { additionalProperties: false }),
  Type.Object({
    op: Type.Literal('move_subtree'),
    id: nodeId,
    new_parent_id: Type.Optional(Type.Union([nodeId, Type.Null()])),
  }, { additionalProperties: false }),
  Type.Object({
    op: Type.Literal('delete_node'),
    id: nodeId,
    cascade: Type.Optional(Type.Boolean()),
  }, { additionalProperties: false }),
  Type.Object({
    op: Type.Literal('add_edge'),
    id: Type.Optional(edgeId),
    from_node: nodeId,
    to_node: nodeId,
    kind: Type.Optional(Type.Union(['hierarchy', 'reference'].map(v => Type.Literal(v)))),
    label: Type.Optional(Type.String()),
    color: Type.Optional(Type.String()),
  }, { additionalProperties: false }),
  Type.Object({
    op: Type.Literal('delete_edge'),
    id: edgeId,
  }, { additionalProperties: false }),
]);

function toolResult(result) {
  return {
    content: [{ type: 'text', text: JSON.stringify(result) }],
    details: result,
    isError: result.status !== 'ok',
  };
}

export function canvasTools(request) {
  return [
    {
      name: 'folio_canvas_read',
      label: '读取知识画布',
      description:
        '读取项目内 .canvas 画布（若已在编辑器打开，直接读取最新内存草稿）。默认 mode="outline" 返回节点文本、父子层级、锚点与连线；需位置信息时传 mode="full"。同时返回 revision 用于并发安全校验。',
      parameters: parameters({
        path,
        mode: Type.Optional(Type.Union([Type.Literal('outline'), Type.Literal('full')])),
      }),
      execute: async (_id, params, signal) => toolResult(await request('read', params, signal)),
    },
    {
      name: 'folio_canvas_get_selection',
      label: '读取当前画布选区',
      description: '读取当前活动 .canvas 画布中选中的节点列表、revision 与空闲区域建议坐标。',
      parameters: parameters({}),
      execute: async (_id, params, signal) => toolResult(await request('get_selection', params ?? {}, signal)),
    },
    {
      name: 'folio_canvas_create',
      label: '新建知识画布',
      description: '在当前项目创建新 .canvas 文件（不会覆盖已有文件），可通过 ops 一次性初始化节点与导图分支。',
      parameters: parameters({
        path,
        ops: Type.Optional(Type.Array(opSchema)),
      }),
      execute: async (_id, params, signal) => toolResult(await request('create', params, signal)),
    },
    {
      name: 'folio_canvas_apply_ops',
      label: '增量修改知识画布',
      description:
        '按事务批量增删改画布节点、父子层级与连线。已打开画布直接进入内存缓冲和单步撤销栈（用户可用 Ctrl+Z 一键撤回）；修改已有画布前应先读取获取 revision 并传入 expected_revision。add_node 指定 parent_id 时自动继承树形连接与建议位置，无需估算像素坐标。',
      parameters: parameters({
        path,
        expected_revision: Type.Optional(Type.String()),
        ops: Type.Array(opSchema, { minItems: 1 }),
      }),
      execute: async (_id, params, signal) => toolResult(await request('apply_ops', params, signal)),
    },
    {
      name: 'folio_canvas_layout',
      label: '整理画布导图布局',
      description: '对指定根节点或整张画布的树形分支执行确定性自动排布，默认跳过用户手动固定（pinned）的节点。',
      parameters: parameters({
        path,
        root_id: Type.Optional(nodeId),
        mode: Type.Optional(Type.Union(['mindmap_right', 'mindmap_left', 'mindmap_balanced', 'tree_down'].map(v => Type.Literal(v)))),
        unpin_subtree: Type.Optional(Type.Boolean()),
      }),
      execute: async (_id, params, signal) => toolResult(await request('layout', params, signal)),
    },
  ];
}
