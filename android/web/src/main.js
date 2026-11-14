import 'katex/dist/katex.min.css';
import 'prosemirror-view/style/prosemirror.css';
import './style.css';
import { createEditor, drawMath } from './editor.js';

const demo = String.raw`# 从这里开始写

这是你的随身 Markdown 笔记。直接点击正文编辑，点击公式修改 LaTeX。

## 两种写法，一样自然

行内公式支持美元符号 $ E = mc^2 $，也支持括号 \( a^2 + b^2 = c^2 \)。分隔符内的空格可以保留。

\[
\int_0^1 x^2\,\mathrm{d}x = \frac{1}{3}
\]

$$
\sum_{k=1}^{n} k = \frac{n(n+1)}{2}
$$

## 记录你的想法

- 从文件打开 Markdown，或把内容直接粘贴进来。
- 用底部工具栏插入公式、标题和列表。
- 笔记自动保存在本机，点右上角导出为 Markdown。

> 写作时专注内容，阅读时看见公式。

代码中的分隔符保持原样：\`$ x $\`。
`.replaceAll('\\`', '`');

const storageKey = 'folio.android.documents.v1';
let records;
try { records = JSON.parse(localStorage.getItem(storageKey)); } catch { records = null; }
if (!Array.isArray(records) || !records.length) records = [{ id: crypto.randomUUID(), name: '欢迎使用 Folio', markdown: demo }];
let active = records.find((record) => record.id === localStorage.getItem('folio.android.active')) || records[0];
let sourceMode = false;
let readMode = false;
let editor;
let formulaPos = null;
let formulaOriginal = null;
let toastTimer;

document.querySelector('#app').innerHTML = `
  <header class="app-header">
    <button id="library-button" class="brand" aria-label="打开笔记列表"><span class="brand-mark" aria-hidden="true">F</span><span>Folio</span></button>
    <div class="header-actions">
      <button id="read-button" aria-label="切换阅读模式" aria-pressed="false">阅读</button>
      <button id="source-button" aria-label="切换源码模式" aria-pressed="false">源码</button>
      <button id="export-button" class="primary">导出</button>
    </div>
  </header>
  <main id="main">
    <div class="document-meta"><input id="document-title" aria-label="笔记名称" placeholder="未命名"><span id="save-status" role="status">已保存在本机</span></div>
    <div id="visual-editor"></div>
    <textarea id="source-editor" aria-label="Markdown 源码" spellcheck="false" hidden></textarea>
    <p id="reading-hint" hidden>阅读模式 · 点击右上角“编辑”继续写作</p>
  </main>
  <footer class="toolbar" aria-label="编辑工具栏">
    <button data-command="undo" aria-label="撤销">↶</button>
    <button data-command="redo" aria-label="重做">↷</button>
    <span class="toolbar-separator" aria-hidden="true"></span>
    <button data-command="heading" aria-label="二级标题">H₂</button>
    <button data-command="bold" aria-label="粗体"><b>B</b></button>
    <button data-command="italic" aria-label="斜体"><i>I</i></button>
    <button data-command="list" aria-label="项目列表">☷</button>
    <button data-command="code" aria-label="代码块">&lt;/&gt;</button>
    <button id="formula-button" aria-label="插入数学公式">ƒx</button>
  </footer>
  <dialog id="formula-dialog" aria-labelledby="formula-title">
    <form id="formula-form">
      <div class="dialog-heading"><h2 id="formula-title">编辑公式</h2><button type="button" data-close="formula-dialog" aria-label="关闭公式编辑">×</button></div>
      <label for="formula-style">公式格式</label>
      <select id="formula-style">
        <option value="dollar-inline">行内 · $ … $</option><option value="tex-inline">行内 · \\( … \\)</option>
        <option value="dollar-block">独立公式 · $$ … $$</option><option value="tex-block">独立公式 · \\[ … \\]</option>
      </select>
      <label for="formula-source">LaTeX 源码</label>
      <textarea id="formula-source" rows="4" spellcheck="false" autocapitalize="off" autocomplete="off"></textarea>
      <div id="formula-preview" aria-label="公式预览"></div>
      <div class="dialog-actions"><button type="button" data-close="formula-dialog">取消</button><button class="primary" type="submit">应用公式</button></div>
    </form>
  </dialog>
  <dialog id="library-dialog" aria-labelledby="library-title">
    <div class="dialog-heading"><h2 id="library-title">本机笔记</h2><button data-close="library-dialog" aria-label="关闭笔记列表">×</button></div>
    <div class="library-actions"><button id="new-button" class="primary">新建笔记</button><button id="open-button">打开 Markdown</button></div>
    <div id="document-list"></div>
    <p class="subtle">笔记保存在当前设备。导出文件后可备份或在其他设备继续编辑。</p>
  </dialog>
  <dialog id="export-dialog" aria-labelledby="export-title">
    <div class="dialog-heading"><h2 id="export-title">导出笔记</h2><button data-close="export-dialog" aria-label="关闭导出">×</button></div>
    <button id="save-file-button" class="wide-action">保存 Markdown 文件</button>
    <button id="share-button" class="wide-action">分享 Markdown 文件</button>
  </dialog>
  <input id="file-input" type="file" accept=".md,.markdown,.txt,text/plain,text/markdown" hidden>
  <div id="toast" role="status" hidden></div>
`;

const $ = (id) => document.getElementById(id);
function notify(message) {
  $('toast').textContent = message; $('toast').hidden = false;
  clearTimeout(toastTimer); toastTimer = setTimeout(() => { $('toast').hidden = true; }, 3500);
}
function persist() {
  try {
    localStorage.setItem(storageKey, JSON.stringify(records));
    localStorage.setItem('folio.android.active', active.id);
    $('save-status').textContent = '已保存在本机';
  } catch {
    $('save-status').textContent = '本机存储已满，请导出备份';
  }
}
function updateSource(markdown) { active.markdown = markdown; persist(); }
function loadDocument(record) {
  active = record;
  $('document-title').value = active.name;
  $('source-editor').value = active.markdown;
  editor.load(active.markdown);
  persist();
  $('library-dialog').close();
  $('main').scrollTop = 0;
}

const styles = {
  'dollar-inline': { open: '$', close: '$', display: false },
  'tex-inline': { open: '\\(', close: '\\)', display: false },
  'dollar-block': { open: '$$', close: '$$', display: true },
  'tex-block': { open: '\\[', close: '\\]', display: true },
};
function formulaAttrs() {
  const style = styles[$('formula-style').value];
  // Keep imported fences, body whitespace and copied escaping when only editing TeX.
  const unchangedStyle = formulaOriginal && formulaOriginal.display === style.display &&
    (formulaOriginal.open.includes('$') === style.open.includes('$'));
  return { ...(unchangedStyle ? formulaOriginal : style), latex: $('formula-source').value };
}
function previewFormula() { drawMath($('formula-preview'), formulaAttrs()); }
function openFormula(node = null, pos = null) {
  if (readMode) return;
  formulaPos = pos;
  formulaOriginal = node?.attrs ?? null;
  $('formula-source').value = node?.attrs.latex ?? '';
  $('formula-style').value = node ? `${node.attrs.open.includes('$') ? 'dollar' : 'tex'}-${node.attrs.display ? 'block' : 'inline'}` : 'dollar-block';
  $('formula-style').disabled = node !== null;
  $('formula-title').textContent = node ? '编辑公式' : '插入公式';
  previewFormula(); $('formula-dialog').showModal(); $('formula-source').focus();
}
editor = createEditor($('visual-editor'), active.markdown, { onChange: updateSource, onFormula: openFormula });
$('document-title').value = active.name;
$('source-editor').value = active.markdown;
persist();

$('document-title').addEventListener('input', () => { active.name = $('document-title').value; persist(); });
$('source-editor').addEventListener('input', () => updateSource($('source-editor').value));
$('source-button').onclick = () => {
  sourceMode = !sourceMode;
  if (sourceMode) $('source-editor').value = active.markdown;
  else editor.load(active.markdown);
  $('visual-editor').hidden = sourceMode; $('source-editor').hidden = !sourceMode;
  $('source-button').setAttribute('aria-pressed', String(sourceMode));
  $('formula-button').disabled = sourceMode || readMode;
  document.querySelectorAll('[data-command]').forEach((button) => { button.disabled = sourceMode || readMode; });
};
$('read-button').onclick = () => {
  readMode = !readMode;
  editor.view.setProps({ editable: () => !readMode });
  $('source-editor').readOnly = readMode;
  $('document-title').readOnly = readMode;
  $('read-button').textContent = readMode ? '编辑' : '阅读';
  $('read-button').setAttribute('aria-pressed', String(readMode));
  $('reading-hint').hidden = !readMode;
  document.querySelector('.toolbar').hidden = readMode;
};
document.querySelectorAll('[data-command]').forEach((button) => {
  // Preserve the editor selection when a touch toolbar steals focus.
  button.onpointerdown = (e) => e.preventDefault();
  button.onclick = () => editor.command(button.dataset.command);
});
$('formula-button').onclick = () => openFormula();
$('formula-source').oninput = previewFormula;
$('formula-style').onchange = previewFormula;
$('formula-form').onsubmit = (e) => {
  e.preventDefault();
  const attrs = formulaAttrs();
  if (!attrs.latex.trim()) { $('formula-source').focus(); return; }
  $('formula-dialog').close();
  editor.setFormula(formulaPos, attrs, attrs.display);
};
document.querySelectorAll('[data-close]').forEach((button) => { button.onclick = () => $(button.dataset.close).close(); });
$('library-button').onclick = () => {
  $('document-list').replaceChildren(...records.map((record) => {
    const button = document.createElement('button');
    button.className = `document-item${record === active ? ' active' : ''}`;
    button.textContent = record.name.trim() || '未命名';
    button.onclick = () => loadDocument(record);
    return button;
  }));
  $('library-dialog').showModal();
};
$('new-button').onclick = () => {
  const record = { id: crypto.randomUUID(), name: '未命名', markdown: '' };
  records.unshift(record); loadDocument(record);
  if (!sourceMode && !readMode) editor.view.focus();
};
window.folioOpenDocument = ({ name, markdown }) => {
  const record = { id: crypto.randomUUID(), name: (name || '未命名').replace(/\.(md|markdown|txt)$/i, ''), markdown };
  records.unshift(record); loadDocument(record); notify('已导入，原文件保持不变');
};
window.folioNativeError = (message) => notify(message);
window.folioNativeSaved = () => notify('文件已导出');
$('open-button').onclick = () => {
  if (window.FolioNative) window.FolioNative.openDocument();
  else $('file-input').click();
};
$('file-input').onchange = async () => {
  const file = $('file-input').files[0];
  if (file) window.folioOpenDocument({ name: file.name, markdown: await file.text() });
  $('file-input').value = '';
};
$('export-button').onclick = () => $('export-dialog').showModal();
function filename() { return `${(active.name.trim() || '未命名').replace(/[\\/:*?"<>|]/g, '_')}.md`; }
function download() {
  const url = URL.createObjectURL(new Blob([active.markdown], { type: 'text/markdown;charset=utf-8' }));
  const a = document.createElement('a'); a.href = url; a.download = filename(); a.click();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}
$('save-file-button').onclick = () => {
  $('export-dialog').close();
  if (window.FolioNative) window.FolioNative.saveDocument(filename(), active.markdown);
  else download();
};
$('share-button').onclick = async () => {
  $('export-dialog').close();
  if (window.FolioNative) window.FolioNative.shareDocument(filename(), active.markdown);
  else {
    const file = new File([active.markdown], filename(), { type: 'text/markdown' });
    if (navigator.canShare?.({ files: [file] })) {
      try { await navigator.share({ files: [file] }); } catch (e) { if (e.name !== 'AbortError') notify('分享失败，请使用导出'); }
    } else download();
  }
};
