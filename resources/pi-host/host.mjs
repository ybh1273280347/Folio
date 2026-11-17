/** Folio's JSONL host. Each retained SDK session owns its subscriptions and UI requests. */
import fs from 'node:fs';
import path from 'node:path';
import os from 'node:os';
import readline from 'node:readline';
import { randomUUID } from 'node:crypto';
import { format } from 'node:util';
import OpenAI from 'openai';
import { createAgentSession, SessionManager, DefaultResourceLoader, ModelRuntime, getAgentDir, initTheme,
  createMcpExtension, createCodemodeExtension, createToolSearchExtension } from '@earendil-works/pi-coding-agent';
import { webTools } from './web-tools.mjs';

// stdout is a protocol channel, including while third-party extensions initialize.
console.log = (...args) => console.error(format(...args));
// SDK exports initialization but not the live theme. The host pins Pi's version for this UI adapter.
initTheme('dark', false);
const { theme } = await import(new URL('./modes/interactive/theme/theme.js', import.meta.resolve('@earendil-works/pi-coding-agent')));
const send = value => process.stdout.write(`${JSON.stringify(value)}\n`);
const root = process.env.FOLIO_USER_DATA ?? path.join(os.homedir(), 'AppData', 'Roaming', 'folio-gpui');
fs.mkdirSync(root, { recursive: true });
const metadataPath = path.join(root, 'ai-session-metadata.json');
const metadata = fs.existsSync(metadataPath) ? JSON.parse(fs.readFileSync(metadataPath, 'utf8')) : {};
const sessions = new Map();
const opening = new Map();
const projectKey = cwd => fs.realpathSync.native(cwd).replaceAll('/', '\\').replace(/\\+$/, '').toLowerCase();
const key = (project, id) => JSON.stringify([project, id]);
let settings = JSON.parse(process.env.FOLIO_AI_SETTINGS ?? '{}');

function saveMetadata() {
  const temporary = `${metadataPath}.tmp`;
  fs.writeFileSync(temporary, JSON.stringify(metadata, null, 2));
  fs.renameSync(temporary, metadataPath);
}

function publish(record, event) {
  send({ ...event, projectKey: record.project, sessionId: record.id, sequence: ++record.sequence });
}

function state(record) {
  const session = record.session;
  return { sessionId: record.id, sessionFile: session.sessionFile, sessionName: session.sessionName,
    isStreaming: record.running, isCompacting: session.isCompacting, model: session.model,
    thinkingLevel: session.thinkingLevel, waiting: record.pending.size > 0, failed: record.failed };
}

function uiContext(record) {
  const request = (method, fields, options) => new Promise(resolve => {
    if (options?.signal?.aborted) { resolve(undefined); return; }
    const id = randomUUID();
    let timer;
    const abort = () => finish(undefined);
    const finish = value => {
      clearTimeout(timer);
      options?.signal?.removeEventListener('abort', abort);
      record.pending.delete(id);
      publish(record, { type: 'session_status', ...state(record) });
      const next = record.pending.entries().next().value;
      if (next) publish(record, { type: 'extension_ui_request', id: next[0], method: next[1].method, ...next[1].fields });
      resolve(value);
    };
    record.pending.set(id, { finish, method, fields });
    // GPUI displays one dialog per conversation; queued parallel tool approvals remain addressable.
    if (record.pending.size === 1) publish(record, { type: 'extension_ui_request', id, method, ...fields });
    publish(record, { type: 'session_status', ...state(record) });
    options?.signal?.addEventListener('abort', abort, { once: true });
    if (options?.timeout) timer = setTimeout(abort, options.timeout);
  });
  const notice = (method, fields) => publish(record, { type: 'extension_ui_request', id: randomUUID(), method, ...fields });
  return {
    select: (title, options, opts) => request('select', { title, options }, opts),
    confirm: async (title, message, opts) => Boolean(await request('confirm', { title, message }, opts)),
    input: (title, placeholder, opts) => request('input', { title, placeholder }, opts),
    editor: (title, prefill) => request('editor', { title, prefill }),
    notify: (message, notifyType) => notice('notify', { message, notifyType }),
    setStatus: (statusKey, statusText) => notice('setStatus', { statusKey, statusText }),
    setWidget: (widgetKey, lines) => {
      if (lines !== undefined && !Array.isArray(lines)) throw new Error('Folio only supports text extension widgets');
      notice('setWidget', { widgetKey, widgetLines: lines });
    },
    setEditorText: text => notice('set_editor_text', { text }),
    pasteToEditor: text => notice('set_editor_text', { text }),
    getEditorText: () => record.editorText,
    setTitle: () => {}, onTerminalInput: () => () => {},
    setWorkingMessage: () => {}, setWorkingVisible: () => {}, setWorkingIndicator: () => {},
    setHiddenThinkingLabel: () => {}, setFooter: () => {}, setHeader: () => {},
    setEditorComponent: () => {}, addAutocompleteProvider: () => {},
    getEditorComponent: () => undefined,
    theme, getAllThemes: () => [], getTheme: () => undefined,
    setTheme: () => ({ success: false, error: 'Theme switching is managed by Folio' }),
    getToolsExpanded: () => false, setToolsExpanded: () => {},
    custom: async () => { throw new Error('Terminal components are unavailable in Folio'); },
  };
}

async function configureModels(record) {
  const baseUrl = `${settings.base_url.trim().replace(/\/+$/, '').replace(/\/v1$/, '')}/v1`;
  const modelsFile = path.join(process.env.PI_CODING_AGENT_DIR ?? path.join(os.homedir(), '.pi', 'agent'), 'models.json');
  const configured = fs.existsSync(modelsFile) ? JSON.parse(fs.readFileSync(modelsFile, 'utf8')).providers?.magpie : undefined;
  let models = configured?.models;
  if (!models?.length) {
    const response = await fetch(`${baseUrl}/models`, { signal: AbortSignal.timeout(20000) });
    if (!response.ok) throw new Error(`Magpie models: HTTP ${response.status}`);
    models = (await response.json()).data.map(m => ({ id: m.id, name: m.name ?? m.id,
      reasoning: true, input: ['text', 'image'], contextWindow: 200000, maxTokens: 32768,
      cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 } }));
  }
  record.models.registerProvider('magpie', { ...configured, baseUrl, apiKey: 'magpie', api: 'openai-completions', models });
  if (record.session?.model?.provider === 'magpie' && !record.running) {
    await record.session.setModel(record.models.getModel('magpie', record.session.model.id));
  }
}

async function createRecord(cwd, manager) {
  const project = projectKey(cwd);
  if (projectKey(manager.getCwd()) !== project) throw new Error('会话不属于当前项目');
  const id = manager.getSessionId();
  const identity = key(project, id);
  if (sessions.has(identity)) return sessions.get(identity);
  const record = { project, id, cwd, manager, sequence: 0, running: false, failed: false,
    pending: new Map(), editorText: '', retained: true, models: await ModelRuntime.create() };
  await configureModels(record);
  const loader = new DefaultResourceLoader({ cwd, agentDir: getAgentDir(), extensionFactories: [
    createCodemodeExtension({ mode: 'on' }), createToolSearchExtension(), createMcpExtension(),
    pi => pi.on('tool_call', async event => {
      if (!['bash', 'powershell'].includes(event.toolName)) return;
      const ok = await uiContext(record).confirm(`执行 ${event.toolName} 命令`, String(event.input.command));
      if (!ok) return { block: true, reason: '用户拒绝执行命令' };
    }),
  ] });
  await loader.reload();
  const { session } = await createAgentSession({ cwd, sessionManager: manager, modelRuntime: record.models,
    resourceLoader: loader, customTools: webTools(() => settings, cwd, root) });
  record.session = session;
  record.unsubscribe = session.subscribe(event => {
    if (event.type === 'message_update' && event.assistantMessageEvent?.partial) record.partial = event.assistantMessageEvent.partial;
    if (event.type === 'message_end') record.partial = undefined;
    if (event.type === 'agent_start') record.running = true;
    if (event.type === 'agent_settled') {
      record.running = false;
      record.failed = session.messages.at(-1)?.stopReason === 'error';
    }
    publish(record, event);
    if (event.type === 'agent_settled') {
      publish(record, { type: 'session_status', ...state(record) });
      if (!record.retained && !record.titleController) disposeRecord(record);
    }
  });
  sessions.set(identity, record);
  await session.bindExtensions({ mode: 'rpc', uiContext: uiContext(record),
    abortHandler: () => { void session.abort(); },
    shutdownHandler: () => { void session.abort(); },
    onError: error => publish(record, { type: 'extension_error', error: String(error.error ?? error) }),
  });
  return record;
}

function disposeRecord(record) {
  record.unsubscribe();
  record.session.dispose();
  sessions.delete(key(record.project, record.id));
}

async function list(cwd) {
  const project = projectKey(cwd);
  const entries = (await SessionManager.listAll()).filter(entry => entry.cwd && fs.existsSync(entry.cwd) && projectKey(entry.cwd) === project);
  // SessionInfo also contains allMessagesText; never send full transcripts in a history listing.
  const result = new Map(entries.map(entry => [entry.id, { id: entry.id, path: entry.path,
    name: entry.name, firstMessage: entry.firstMessage, messageCount: entry.messageCount,
    modified: entry.modified, projectKey: project, pinned: Boolean(metadata[key(project, entry.id)]?.pinned) }]));
  for (const record of sessions.values()) {
    if (record.project !== project) continue;
    result.set(record.id, { ...result.get(record.id), id: record.id, path: record.session.sessionFile,
      name: record.session.sessionName, firstMessage: result.get(record.id)?.firstMessage ?? record.preview ?? '',
      messageCount: record.session.messages.length, modified: result.get(record.id)?.modified ?? new Date().toISOString(),
      pinned: Boolean(metadata[key(project, record.id)]?.pinned), ...state(record) });
  }
  return [...result.values()].sort((a, b) => Number(b.pinned) - Number(a.pinned) || new Date(b.modified) - new Date(a.modified));
}

async function target(command) {
  const project = projectKey(command.cwd);
  if (command.projectKey !== project) throw new Error('项目标识不匹配');
  const identity = key(project, command.sessionId);
  if (sessions.has(identity)) return sessions.get(identity);
  if (opening.has(identity)) return opening.get(identity);
  const entry = (await list(command.cwd)).find(entry => entry.id === command.sessionId);
  if (!entry) throw new Error('找不到当前项目的会话');
  const promise = createRecord(command.cwd, SessionManager.open(entry.path));
  opening.set(identity, promise);
  try { return await promise; } finally { opening.delete(identity); }
}

async function generateTitle(record, text) {
  const identity = key(record.project, record.id);
  if (metadata[identity]?.titleSource === 'manual' || record.titleController) return;
  const controller = new AbortController();
  record.titleController = controller;
  try {
    const client = new OpenAI({ baseURL: `${settings.base_url.replace(/\/+$/, '').replace(/\/v1$/, '')}/v1`,
      apiKey: 'magpie', timeout: 30000, maxRetries: 0 });
    const response = await client.chat.completions.create({ model: settings.title_model_id,
      messages: [{ role: 'system', content: '为用户内容生成一个简短中文会话标题，最多16个汉字。只输出标题，不加引号。' },
        { role: 'user', content: text.slice(0, 2000) }], stream: false }, { signal: controller.signal });
    const title = [...(response.choices[0].message.content ?? '').trim().replace(/^["“”'「」]+|["“”'「」]+$/g, '').replace(/\s+/g, ' ')].slice(0, 24).join('');
    if (!title) throw new Error('标题模型返回空内容');
    // A manual rename or deletion can finish while the separate model request is pending.
    if (!sessions.has(identity) || metadata[identity]?.titleSource === 'manual') return;
    record.session.setSessionName(title);
    metadata[identity] = { ...metadata[identity], titleSource: 'generated' };
    saveMetadata();
    publish(record, { type: 'session_info_changed', name: title });
  } catch (error) {
    if (!controller.signal.aborted) publish(record, { type: 'title_error', error: error.message });
  } finally {
    record.titleController = undefined;
    if (!record.retained && !record.running && sessions.has(identity)) disposeRecord(record);
  }
}

async function dispatch(command) {
  const response = data => send({ type: 'response', requestId: command.requestId, routeId: command.routeId,
    projectKey: command.projectKey, sessionId: command.sessionId, command: command.type, success: true, data: data ?? {} });
  if (command.type === 'configure') {
    settings = command.settings;
    await Promise.all([...sessions.values()].map(configureModels));
    return response();
  }
  if (command.projectKey !== projectKey(command.cwd)) throw new Error('项目标识不匹配');
  if (command.type === 'list_sessions') return response({ sessions: await list(command.cwd) });
  if (command.type === 'create_session') {
    const record = await createRecord(command.cwd, SessionManager.create(command.cwd));
    return response(state(record));
  }
  if (command.type === 'set_pinned') {
    const entries = await list(command.cwd);
    if (!entries.some(entry => entry.id === command.sessionId)) throw new Error('找不到当前项目的会话');
    const identity = key(command.projectKey, command.sessionId);
    metadata[identity] = { ...metadata[identity], pinned: command.pinned };
    saveMetadata();
    return response();
  }
  const record = await target(command);
  const session = record.session;
  switch (command.type) {
    case 'snapshot': {
      record.retained = true;
      const messages = record.partial && session.messages.at(-1)?.role !== 'assistant'
        ? [...session.messages, record.partial] : session.messages;
      response({ ...state(record), messages, sequence: record.sequence });
      const next = record.pending.entries().next().value;
      if (next) publish(record, { type: 'extension_ui_request', id: next[0], method: next[1].method, ...next[1].fields });
      return;
    }
    case 'get_state': return response(state(record));
    case 'get_messages': return response({ messages: session.messages });
    case 'get_session_stats': return response({ ...session.getSessionStats(), contextUsage: session.getContextUsage() });
    case 'get_available_models': return response({ models: await record.models.getAvailable() });
    case 'get_available_thinking_levels': return response({ levels: session.getAvailableThinkingLevels() });
    case 'get_commands': return response({ commands: [
      ...session.extensionRunner.getRegisteredCommands().map(c => ({ name: c.invocationName, description: c.description, source: 'extension' })),
      ...session.promptTemplates.map(c => ({ name: c.name, description: c.description, source: 'prompt' })),
      ...session.resourceLoader.getSkills().skills.map(c => ({ name: `skill:${c.name}`, description: c.description, source: 'skill' })),
    ] });
    case 'prompt': {
      if (record.running) throw new Error('会话正在运行');
      record.running = true;
      record.failed = false;
      record.editorText = command.message;
      const first = !session.messages.some(m => m.role === 'user');
      record.preview = command.titleText ?? command.message.slice(0, 48);
      const task = session.prompt(command.message, { images: command.images });
      if (first && command.titleText?.trim() && settings.title_model_id) {
        metadata[key(record.project, record.id)] = { ...metadata[key(record.project, record.id)], titleSource: 'pending', titleText: command.titleText.slice(0, 2000) };
        saveMetadata();
        void generateTitle(record, command.titleText);
      }
      publish(record, { type: 'session_status', ...state(record) });
      try { await task; } finally {
        record.running = false;
        publish(record, { type: 'session_status', ...state(record) });
      }
      return response();
    }
    case 'abort':
      for (const pending of record.pending.values()) pending.finish(undefined);
      await session.abort();
      return response();
    case 'extension_ui_response': {
      const pending = record.pending.get(command.id);
      if (!pending) throw new Error('确认请求已结束');
      pending.finish(pending.method === 'confirm' ? command.confirmed : command.cancelled ? undefined : command.value);
      return response();
    }
    case 'set_model': {
      const model = record.models.getModel(command.provider, command.modelId);
      if (!model) throw new Error('找不到模型');
      await session.setModel(model);
      return response(model);
    }
    case 'set_thinking_level': session.setThinkingLevel(command.level); return response();
    case 'compact': return response(await session.compact());
    case 'set_session_name': {
      const name = command.name.trim();
      if (!name) throw new Error('会话名称不能为空');
      metadata[key(record.project, record.id)] = { ...metadata[key(record.project, record.id)], titleSource: 'manual' };
      saveMetadata();
      session.setSessionName(name);
      publish(record, { type: 'session_info_changed', name });
      return response();
    }
    case 'regenerate_title': {
      const text = session.messages.find(m => m.role === 'user')?.content;
      const body = metadata[key(record.project, record.id)]?.titleText
        ?? (typeof text === 'string' ? text : text?.filter(b => b.type === 'text').map(b => b.text).join(' '));
      if (!body) throw new Error('会话尚无用户消息');
      metadata[key(record.project, record.id)] = { ...metadata[key(record.project, record.id)], titleSource: 'pending' };
      saveMetadata();
      void generateTitle(record, body);
      return response();
    }
    case 'release':
      record.retained = false;
      if (!record.running && !record.titleController) disposeRecord(record);
      return response();
    case 'retain': record.retained = true; return response();
    case 'delete_session': {
      if ((record.running || record.pending.size) && !command.confirmed) throw new Error('删除运行中的会话需要确认');
      record.titleController?.abort();
      for (const pending of record.pending.values()) pending.finish(undefined);
      await session.abort();
      const file = session.sessionFile;
      disposeRecord(record);
      if (file && fs.existsSync(file)) fs.unlinkSync(file);
      delete metadata[key(record.project, record.id)];
      saveMetadata();
      return response();
    }
    default: throw new Error(`未知宿主操作: ${command.type}`);
  }
}

const input = readline.createInterface({ input: process.stdin, crlfDelay: Infinity });
input.on('line', line => {
  let command;
  try { command = JSON.parse(line); } catch (error) { console.error(error.message); return; }
  // Request-level errors are reported to the caller; other sessions keep their subscriptions.
  void dispatch(command).catch(error => {
    console.error(error.stack);
    send({ type: 'response', requestId: command.requestId,
      routeId: command.routeId, projectKey: command.projectKey, sessionId: command.sessionId,
      command: command.type, success: false, error: error.message });
  });
});
input.on('close', () => {
  for (const record of sessions.values()) {
    record.titleController?.abort();
    for (const pending of record.pending.values()) pending.finish(undefined);
    record.session.dispose();
  }
  process.exit(0);
});
