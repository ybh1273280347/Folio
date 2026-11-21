/** Read and write native Draw.io XML through the active Folio workspace. */
import { Type } from '@sinclair/typebox';

const path = Type.String({ minLength: 1, description: '当前 Folio 项目内的 .drawio 相对路径。' });
const xml = Type.String({ minLength: 1, description: '完整的、以 mxfile 为根元素的 Draw.io XML。' });
const parameters = fields => Type.Object(fields, { additionalProperties: false });

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

  return [read, create, update];
}
