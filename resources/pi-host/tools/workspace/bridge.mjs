/** Route workspace tool calls across JSONL, including cancellations from the SDK. */
import { randomUUID } from 'node:crypto';
import { ToolFailure } from '../tool-failure.mjs';

export const cancelled = () => ({ status: 'cancelled', error: { code: 'cancelled', message: '操作已取消。' } });

function classifyFailure(result, operation, retryableForError) {
  if (result.status !== 'error' || typeof result.error.retryable === 'boolean') return result;
  return {
    ...result,
    error: { ...result.error, retryable: retryableForError(operation, result.error) },
  };
}

export class WorkspaceToolBridge {
  #pending = new Map();
  constructor(send, channel, {
    timeoutMs = 30_000,
    timeouts = {},
    timeoutMessage = `${channel} 工具请求超时。`,
    retryableForError = () => false,
  } = {}) {
    this.send = send;
    this.channel = channel;
    this.timeoutMs = timeoutMs;
    this.timeouts = timeouts;
    this.timeoutMessage = timeoutMessage;
    this.retryableForError = retryableForError;
  }

  request(owner, operation, params, signal) {
    if (signal?.aborted) return Promise.resolve(cancelled());
    return new Promise(resolve => {
      const id = randomUUID();
      let timer;
      const finish = result => {
        clearTimeout(timer);
        signal?.removeEventListener('abort', abort);
        this.#pending.delete(id);
        resolve(result);
      };
      const abort = () => {
        this.send({ type: 'workspace_tool_cancel', channel: this.channel, id, ...owner });
        finish(cancelled());
      };
      this.#pending.set(id, { ...owner, operation, finish });
      signal?.addEventListener('abort', abort, { once: true });
      const timeoutMs = this.timeouts[operation] ?? this.timeoutMs;
      timer = setTimeout(() => {
        const failure = new ToolFailure('workspace_tool_timeout', this.timeoutMessage,
          this.retryableForError(operation, { code: 'workspace_tool_timeout' }));
        finish(failure.toResult());
        this.send({ type: 'workspace_tool_cancel', channel: this.channel, id, ...owner });
      }, timeoutMs);
      this.send({ type: 'workspace_tool_request', channel: this.channel, id, ...owner, operation, params });
    });
  }

  respond(command) {
    const pending = this.#pending.get(command.id);
    // Late replies cannot reach a different project or a replacement conversation.
    if (!pending) return;
    if (pending.projectKey !== command.projectKey || pending.sessionId !== command.sessionId) {
      throw new Error('工具应答的会话归属不匹配');
    }
    pending.finish(classifyFailure(command.result, pending.operation, this.retryableForError));
  }

  dispose(owner) {
    for (const pending of this.#pending.values()) {
      if (pending.projectKey === owner.projectKey && pending.sessionId === owner.sessionId) pending.finish(cancelled());
    }
  }
}
