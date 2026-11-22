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

  release(record) {
    record.retained = false;
    return !record.running && !record.titleController && !record.rewind;
  }

  retain(record) {
    record.retained = true;
  }

  dispose(record, bridges) {
    for (const bridge of bridges) {
      bridge.dispose({ projectKey: record.project, sessionId: record.id });
    }
    record.unsubscribe();
    record.session.dispose();
    this.remove(record);
  }

  prepare(project, manager, created) {
    const identity = this.identity(project, manager.getSessionId());
    this.#prepared.set(identity, { project, manager, created });
    return manager.getSessionId();
  }

  async open(project, sessionId, initialize) {
    const identity = this.identity(project, sessionId);
    const record = this.#records.get(identity);
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
