/** Local knowledge-base search and index management bridged to Folio Workspace. */
import { Type } from '@sinclair/typebox';

const text = Type.String({ minLength: 1 });
const schema = fields => Type.Object(fields, { additionalProperties: false });
const subSearch = schema({
  type: Type.Union([Type.Literal('lex'), Type.Literal('vec')], {
    description: 'lex：关键词/精确术语/LaTeX 宏；vec：自然语言语义向量。',
  }),
  query: text,
});

const resultForModel = result => ({
  content: [{ type: 'text', text: JSON.stringify(result) }],
  details: result,
  isError: result.status !== 'ok',
});

export function ragTools(request) {
  const run = operation => async (_id, params, signal) =>
    resultForModel(await request(operation, params ?? {}, signal));

  return [
    {
      name: 'folio_kb_search',
      label: '检索 Folio 本地知识库',
      description:
        '在当前项目知识库中检索（自动包含未保存编辑器草稿）。通过 searches 中各条目的 type 自动分流：'
        + '仅传 lex 时只做词法检索，仅传 vec 时只做向量检索，同时传则混合 RRF 融合（首条权重 2x）。'
        + '可选 intent 用于指定自然语言检索意图并引导重排打分。'
        + '返回匹配文件路径、章节、行号/页码、片段以及已精确解析 PDF 的 markdownCache 路径。'
        + '读取精修 PDF 文本请优先使用 folio_pdf_read（或直接读取 markdownCache）。',
      parameters: schema({
        searches: Type.Array(subSearch, { minItems: 1, maxItems: 10 }),
        intent: Type.Optional(text),
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
      description: '查看当前项目的本地知识库索引统计（已索引文件数、切片数、向量数、待补向量数、需深度识别 PDF 及本地服务状态）。',
      parameters: schema({}),
      execute: run('status'),
    },
    {
      name: 'folio_kb_reindex',
      label: '更新或重建 Folio 知识库索引',
      description:
        '对当前项目执行增量或全量知识库索引同步。默认走快速本地提取（Fast）按修改时间与 SHA-256 增量更新；'
        + '传 force_rebuild: true 时强制重建；'
        + '传 accurate: "only_ocr"（仅扫描件）或 "all"（全部 PDF）显式触发深度精修解析，可配合 paths 指定单篇或批量 PDF。',
      parameters: schema({
        force_rebuild: Type.Optional(Type.Boolean()),
        accurate: Type.Optional(Type.Union([Type.Literal('only_ocr'), Type.Literal('all')])),
        paths: Type.Optional(Type.Array(text, { minItems: 1, maxItems: 100 })),
      }),
      execute: run('reindex'),
    },
    {
      name: 'folio_pdf_read',
      label: '按页读取 PDF Markdown 文本',
      description:
        '按页读取项目内 PDF 的 Markdown 文本（单次最多 20 页，默认向后读 5 页）。'
        + '若该 PDF 已通过知识库精确解析，优先返回缓存的 PaddleOCR Markdown 及 markdownCache 路径；'
        + '否则用本地 pdf-inspector 快速提取。读取 PDF 文字/公式/表格时应优先使用本工具复用已解析缓存，'
        + '避免重复调用外部 OCR；同时仍可自由使用其他工具操作原 PDF。',
      parameters: schema({
        path: text,
        start_page: Type.Optional(Type.Integer({ minimum: 1 })),
        end_page: Type.Optional(Type.Integer({ minimum: 1 })),
      }),
      execute: run('read_pdf'),
    },
  ];
}
