/** Read-only knowledge graph query tools backed by Folio's Markdown & Canvas link index. */
import { Type } from '@sinclair/typebox';

const relPath = Type.String({
  minLength: 1,
  description: '项目内的文件相对路径（如 concepts/双链与知识图谱.md 或 maps/总览.canvas）。',
});
const schema = fields => Type.Object(fields, { additionalProperties: false });
const resultForModel = result => ({
  content: [{ type: 'text', text: JSON.stringify(result) }],
  details: result,
  isError: result.status !== 'ok',
});

export function graphTools(request) {
  const call = operation => async (_id, params, signal) =>
    resultForModel(await request(operation, params, signal));

  return [
    {
      name: 'folio_graph_neighbors',
      label: '查询图谱邻域与出入链',
      description: '基于当前项目知识图谱索引，一次性查询指定文件（Markdown 或 Canvas）的入链（谁引用了它，含行号或画布节点 ID）、出链（它引用了谁，含待建悬空双链）以及可选的 2 跳关联节点和标签。排查引用上下文或做双链梳理时优先用此工具，比逐个读文件更快且不遗漏 Canvas 锚点。',
      parameters: schema({
        path: relPath,
        depth: Type.Optional(Type.Integer({ minimum: 1, maximum: 2, description: '1 仅返回直接入链/出链（默认）；2 额外返回 two_hop 二跳关联节点。' })),
        include_attachments: Type.Optional(Type.Boolean({ description: '是否包含图片/PDF 等附件节点，默认 false。' })),
      }),
      execute: call('neighbors'),
    },
    {
      name: 'folio_graph_overview',
      label: '查看知识图谱全局概览',
      description: '返回当前项目知识图谱的结构统计（Markdown/Canvas/悬空双链/孤立笔记/连线数）、高连接度枢纽节点（hubs）、无任何连接的孤立笔记（orphans），以及被引用但尚未创建的悬空双链清单（unresolved）。适合整理知识库结构或发现待补充主题。',
      parameters: schema({
        include_attachments: Type.Optional(Type.Boolean({ description: '是否将图片/PDF 等附件计入图谱统计，默认 false。' })),
        limit: Type.Optional(Type.Integer({ minimum: 1, maximum: 100, description: '每个分组最多返回条数，默认 20。' })),
      }),
      execute: call('overview'),
    },
    {
      name: 'folio_graph_path',
      label: '查找知识图谱关联路径',
      description: '在知识图谱中查找两篇笔记或画布之间的最短引用路径及每一步的链接元信息（方向、边类型、行号或画布节点 ID）。用于梳理两个概念之间的脉络联系。',
      parameters: schema({
        from_path: relPath,
        to_path: relPath,
        directed: Type.Optional(Type.Boolean({ description: 'true 仅沿引用出链方向寻路；false（默认）忽略方向寻找最短语义连通路径。' })),
        include_attachments: Type.Optional(Type.Boolean({ description: '是否允许经由附件节点中转，默认 false。' })),
      }),
      execute: call('path'),
    },
  ];
}
