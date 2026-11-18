/** Route workspace tool calls across JSONL, including cancellations from the SDK. */
import { randomUUID } from 'node:crypto';

export const cancelled = () => ({ status: 'cancelled', error: { code: 'cancelled', message: '操作已取消。' } });

export class WorkspaceToolBridge {
  #pending = new Map();
  constructor(send, channel) {
    this.send = send;
    this.channel = channel;
  }

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
        this.send({ type: `${this.channel}_cancel`, id, ...owner });
        finish(cancelled());
      };
      this.#pending.set(id, { ...owner, finish });
      signal?.addEventListener('abort', abort, { once: true });
      this.send({ type: `${this.channel}_request`, id, ...owner, operation, params });
    });
  }

  respond(command) {
    const pending = this.#pending.get(command.id);
    // Late replies cannot reach a different project or a replacement conversation.
    if (!pending) return;
    if (pending.projectKey !== command.projectKey || pending.sessionId !== command.sessionId) {
      throw new Error('工具应答的会话归属不匹配');
    }
    pending.finish(command.result);
  }

  dispose(owner) {
    for (const pending of this.#pending.values()) {
      if (pending.projectKey === owner.projectKey && pending.sessionId === owner.sessionId) pending.finish(cancelled());
    }
  }
}
