/** Folio owns annotation placement and persistence; the SDK only transports requests. */
import { Type } from '@sinclair/typebox';
import { randomUUID } from 'node:crypto';

const cancelled = () => ({ status: 'cancelled', error: { code: 'cancelled', message: '批注操作已取消。' } });

export class AnnotationBridge {
  #pending = new Map();
  constructor(send) { this.send = send; }

  request(owner, operation, params, signal) {
    if (signal?.aborted) return Promise.resolve(cancelled());
    return new Promise(resolve => {
      const id = randomUUID();
      const finish = result => {
        signal?.removeEventListener('abort', abort);
        this.#pending.delete(id);
        resolve(result);
      };
      const abort = () => {
        this.send({ type: 'annotation_cancel', id, ...owner });
        finish(cancelled());
      };
      this.#pending.set(id, { ...owner, finish });
      signal?.addEventListener('abort', abort, { once: true });
      this.send({ type: 'annotation_request', id, ...owner, operation, params });
    });
  }

  respond(command) {
    const pending = this.#pending.get(command.id);
    // A cancelled/disposed request can receive a late UI response; it has no owner left.
    if (!pending) return;
    if (pending.projectKey !== command.projectKey || pending.sessionId !== command.sessionId) {
      throw new Error('批注应答的会话归属不匹配');
    }
    pending.finish(command.result);
  }

  dispose(owner) {
    for (const pending of this.#pending.values()) {
      if (pending.projectKey === owner.projectKey && pending.sessionId === owner.sessionId) pending.finish(cancelled());
    }
  }
}

export function annotationTools(request) {
  const path = Type.String({ minLength: 1, description: '当前活动项目内的 Markdown 或 PDF 路径，可用相对路径或项目内绝对路径。' });
  const execute = operation => async (_id, params, signal) => {
    const result = await request(operation, params, signal);
    return { content: [{ type: 'text', text: JSON.stringify(result) }], details: result, isError: result.status !== 'ok' };
  };
  return [
    { name: 'folio_annotations_list', label: '查看 Folio 批注',
      description: '读取当前项目文档已有的 Folio 高亮和评论。批注存于 Folio，不是 PDF/Word 原生评论。',
      parameters: Type.Object({ path }, { additionalProperties: false }), execute: execute('list') },
    { name: 'folio_annotation_create', label: '添加 Folio 批注',
      description: '为已阅读的 Markdown 或文字 PDF 添加原文高亮和评论。quote 必须来自文档的可读原文；Markdown 仅支持单个普通文字块，PDF 必须给实际文件页码。不接收坐标，不支持扫描页、公式和图片区域。多处匹配时先查看候选再指定 occurrence，不猜位置；不覆盖已有不同评论。项目切走时请用户切回，失败不得声称已完成。',
      parameters: Type.Object({ path, quote: Type.String({ minLength: 1 }), comment: Type.String({ minLength: 1 }),
        page: Type.Optional(Type.Integer({ minimum: 1, description: '实际 PDF 文件页码，从 1 开始；不是印刷页码或 Markdown 行号。' })),
        occurrence: Type.Optional(Type.Integer({ minimum: 1, description: '匹配原文的第几处，从 1 开始；唯一匹配时省略。' })),
      }, { additionalProperties: false }), execute: execute('create') },
  ];
}
