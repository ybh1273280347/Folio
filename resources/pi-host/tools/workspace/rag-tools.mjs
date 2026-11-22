/** Local knowledge-base search and index management bridged to Folio Workspace. */
import { Type } from '@sinclair/typebox';

const text = Type.String({ minLength: 1 });
const schema = fields => Type.Object(fields, { additionalProperties: false });

const resultForModel = result => ({
  content: [{ type: 'text', text: JSON.stringify(result) }],
  details: result,
  isError: result.status !== 'ok',
});

export function ragTools(request) {
  const run = (operation, needsParams = true) => async (_id, params, signal) =>
    resultForModel(await request(operation, needsParams ? { params } : {}, signal));

  return [
    {
      name: 'folio_kb_search',
      label: '检索 Folio 本地知识库',
      description:
        '在当前项目知识库中做混合检索（SQLite FTS5 词法 + sqlite-vec 语义向量 RRF 融合，自动包含未保存编辑器草稿）。'
        + '返回匹配的文件路径、章节标题、行号或页码及短片段定位；需要完整推导或原文上下文时，请再用文件读取工具回读对应片段。'
        + '可同时提供 lex_queries（1-3 个精确关键词/LaTeX 宏/英文术语）和 vec_queries（1-2 个语义改写）提高召回率。',
      parameters: schema({
        query: text,
        lex_queries: Type.Optional(Type.Array(text, { minItems: 1, maxItems: 5 })),
        vec_queries: Type.Optional(Type.Array(text, { minItems: 1, maxItems: 4 })),
        mode: Type.Optional(Type.Union(['hybrid', 'keyword', 'semantic'].map(v => Type.Literal(v)))),
        rerank: Type.Optional(Type.Boolean()),
        tags: Type.Optional(Type.Array(text, { minItems: 1, maxItems: 20 })),
        match: Type.Optional(Type.Union(['all', 'any'].map(v => Type.Literal(v)))),
        under_directory: Type.Optional(text),
        extensions: Type.Optional(Type.Array(text, { minItems: 1, maxItems: 10 })),
        limit: Type.Optional(Type.Integer({ minimum: 1, maximum: 20 })),
      }),
      execute: run('search'),
    },
    {
      name: 'folio_kb_status',
      label: '查看 Folio 知识库索引状态',
      description: '查看当前项目的本地知识库索引统计（已索引文件数、切片数、向量数、待补向量数、需 OCR PDF 及本地 Embedding 服务连接状态）。',
      parameters: schema({}),
      execute: run('status', false),
    },
    {
      name: 'folio_kb_reindex',
      label: '更新或重建 Folio 知识库索引',
      description: '对当前项目执行增量或全量知识库索引同步。默认按文件修改时间与 SHA-256 增量更新（PDF 默认先走本地快速抽取）；传 force_rebuild: true 时强制重建；传 upgrade_pdfs: true 时显式调用 MinerU API 对未精修的参考 PDF 进行公式与版面精修并持久化缓存。',
      parameters: schema({
        force_rebuild: Type.Optional(Type.Boolean()),
        upgrade_pdfs: Type.Optional(Type.Boolean()),
      }),
      execute: run('reindex'),
    },
  ];
}
