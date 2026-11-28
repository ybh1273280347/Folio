/** Own session identity, initialization deduplication, and prepared tabs for the Pi host. */
export class SessionRegistry {
  #records = new Map();
  #opening = new Map();
  #prepared = new Map();

  identity(project, sessionId) {
    return JSON.stringify([project, sessionId]);
  }

  get(project, sessionId) {
    return this.#records.get(this.identity(project, sessionId));
  }

  records() {
    return this.#records.values();
  }

  register(record) {
    this.#records.set(this.identity(record.project, record.id), record);
  }

  remove(record) {
    this.#records.delete(this.identity(record.project, record.id));
  }

  async release(project, sessionId, bridges) {
    const identity = this.identity(project, sessionId);
    // A close can arrive while a snapshot is opening the session.
    const record = this.#records.get(identity) ?? await this.#opening.get(identity);
    if (!record) return;
    record.retained = false;
    if (!record.running && !record.task && !record.titleController && !record.rewind) {
      await this.dispose(record, bridges);
    }
  }

  retain(record) {
    record.retained = true;
  }

  dispose(record, bridges, targetSessionFile) {
    // Closing, a settling turn and host EOF can all release the same SDK session.
    return record.disposal ??= Promise.resolve().then(async () => {
      record.titleController?.abort();
      for (const pending of record.pending.values()) pending.finish(undefined);
      for (const bridge of bridges) {
        bridge.dispose({ projectKey: record.project, sessionId: record.id });
      }
      await record.binding;
      const { session } = record;
      await session.abort();
      // Pi emits this event with the extension runtime still bound so MCP can await transport close.
      await session.extensionRunner.emit(targetSessionFile
        ? { type: 'session_shutdown', reason: 'resume', targetSessionFile }
        : { type: 'session_shutdown', reason: 'quit' });
      record.unsubscribe();
      session.dispose();
      await record.models.settleCredentialWrites();
      // Keep the rewind record addressable until createRecord installs its replacement.
      if (!targetSessionFile) this.remove(record);
    });
  }

  prepare(project, manager, created) {
    const identity = this.identity(project, manager.getSessionId());
    this.#prepared.set(identity, { project, manager, created });
    return manager.getSessionId();
  }

  async open(project, sessionId, initialize) {
    const identity = this.identity(project, sessionId);
    const record = this.#records.get(identity);
    if (record?.disposal && !record.rewind) {
      await record.disposal;
      return this.open(project, sessionId, initialize);
    }
    if (record) return record;

    const opening = this.#opening.get(identity);
    if (opening) return opening;

    // Concurrent snapshot and model requests share one disk load and SDK session.
    const promise = initialize(this.#prepared.get(identity));
    this.#opening.set(identity, promise);
    try {
      const opened = await promise;
      this.#prepared.delete(identity);
      return opened;
    } finally {
      this.#opening.delete(identity);
    }
  }

  preparedFor(project) {
    return [...this.#prepared.entries()].filter(([, entry]) => entry.project === project);
  }
}
