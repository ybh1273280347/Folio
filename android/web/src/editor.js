/** The visual editor owns one ProseMirror document and undo history.
 * Import, paste and live math recognition all use the same Markdown contract.
 */
import { EditorState, Plugin, TextSelection, NodeSelection } from 'prosemirror-state';
import { EditorView } from 'prosemirror-view';
import { Slice, Fragment } from 'prosemirror-model';
import { baseKeymap, toggleMark, setBlockType, chainCommands, exitCode } from 'prosemirror-commands';
import { keymap } from 'prosemirror-keymap';
import { history, undo, redo } from 'prosemirror-history';
import { inputRules, wrappingInputRule, textblockTypeInputRule } from 'prosemirror-inputrules';
import { splitListItem, liftListItem, sinkListItem, wrapInList } from 'prosemirror-schema-list';
import { tableEditing, goToNextCell } from 'prosemirror-tables';
import katex from 'katex';
import { mathAt, parseMarkdown, renderLatex, schema, toMarkdown } from './markdown.js';

export function drawMath(element, attrs) {
  katex.render(renderLatex(attrs), element, {
    displayMode: attrs.display, throwOnError: false, trust: false,
    strict: 'ignore', maxExpand: 1000, maxSize: 30, output: 'htmlAndMathml',
  });
}

function liveMath() {
  let composing = false;
  let compositionDoc = null;
  let compositionTransactions = [];
  return new Plugin({
    props: { handleDOMEvents: {
      compositionstart(view) {
        composing = true; compositionDoc = view.state.doc; compositionTransactions = [];
        return false;
      },
      compositionend(view) {
        composing = false;
        // Let the browser finish committing Chinese IME text before replacing nodes.
        setTimeout(() => { if (!view.isDestroyed) view.dispatch(view.state.tr.setMeta('folioCommitMath', true)); }, 0);
        return false;
      },
    } },
    appendTransaction(transactions, oldState, state) {
      const commit = transactions.some((tr) => tr.getMeta('folioCommitMath'));
      if (compositionDoc && !commit) compositionTransactions.push(...transactions);
      if (composing || (!commit && !transactions.some((tr) => tr.docChanged))) return null;
      const originalDoc = commit && compositionDoc ? compositionDoc : oldState.doc;
      const changes = commit && compositionDoc ? compositionTransactions : transactions;
      if (commit) { compositionDoc = null; compositionTransactions = []; }
      const tr = state.tr;
      const matches = [];
      const blocks = [];
      function changedRange(from, to) {
        let oldFrom = from, oldTo = to;
        for (const transaction of [...changes].reverse()) {
          const inverse = transaction.mapping.invert();
          oldFrom = inverse.map(oldFrom, 1); oldTo = inverse.map(oldTo, -1);
        }
        // Literal escaped dollars imported as text must not turn into math when
        // the user edits another part of the document or only changes formatting.
        return !originalDoc.slice(oldFrom, oldTo).content.eq(state.doc.slice(from, to).content);
      }
      let run = [];
      function flushRun() {
        let size = 0;
        const starts = run.map(({ node }) => { const start = size; size += node.textContent.length + 1; return start; });
        const source = run.map(({ node }) => node.textContent).join('\n');
        let offset = 0;
        for (let i = 0; i < run.length; i++) {
          const leading = run[i].node.textContent.match(/^\s*/)[0].length;
          const match = mathAt(source, offset + leading);
          if (match?.display) {
            let end = offset + run[i].node.textContent.length;
            let last = i;
            while (last + 1 < run.length && end < match.end) end += 1 + run[++last].node.textContent.length;
            if (end >= match.end && !source.slice(match.end, end).trim()) {
              const from = run[i].pos, to = run[last].pos + run[last].node.nodeSize;
              if (changedRange(from, to)) blocks.push({ from, to, attrs: match });
              i = last;
            }
          }
          // The next paragraph may follow several formula lines consumed above.
          offset = starts[i + 1];
        }
        run = [];
      }
      // Enter creates sibling paragraphs. Rejoin only plain paragraphs so a
      // multiline display formula cannot swallow code or formatted content.
      state.doc.forEach((node, pos) => {
        if (node.type === schema.nodes.paragraph && (node.childCount === 0 ||
          (node.childCount === 1 && node.firstChild.isText && !node.firstChild.marks.length))) run.push({ node, pos });
        else flushRun();
      });
      flushRun();
      state.doc.descendants((node, pos, parent) => {
        if (!node.isText || parent.type.spec.code || node.marks.some((m) => m.type === schema.marks.code)) return;
        for (let i = 0; i < node.text.length; i++) {
          const match = mathAt(node.text, i);
          if (match) {
            if (changedRange(pos + i, pos + match.end) && !blocks.some((block) => pos + i >= block.from && pos + match.end <= block.to))
              matches.push({ ...match, from: pos + i, to: pos + match.end });
            i = match.end - 1;
          }
        }
      });
      // Work backwards so replacement offsets still address the original document.
      const replacements = [
        ...blocks.map((block) => ({ ...block, node: schema.nodes.math_block.create(block.attrs) })),
        ...matches.map((match) => ({ ...match, node: schema.nodes.math_inline.create(match) })),
      ].sort((a, b) => b.from - a.from);
      for (const match of replacements) {
        tr.replaceWith(match.from, match.to, match.node);
      }
      // Atomic display blocks need a real text position after paste/typing.
      // Without this paragraph the next formula insertion replaces the selected one.
      if (tr.doc.lastChild?.type === schema.nodes.math_block) {
        const end = tr.doc.content.size;
        tr.insert(end, schema.nodes.paragraph.create());
        if (tr.selection instanceof NodeSelection || tr.selection.to >= end - 1)
          tr.setSelection(TextSelection.create(tr.doc, end + 1));
      }
      return tr.docChanged ? tr : null;
    },
  });
}

export function createEditor(host, source, { onChange, onFormula }) {
  const mathView = (node, view, getPos) => {
    const dom = document.createElement(node.isInline ? 'span' : 'div');
    dom.className = `formula ${node.isInline ? 'formula-inline' : 'formula-block'}`;
    dom.contentEditable = 'false';
    dom.tabIndex = 0;
    dom.setAttribute('role', 'button');
    dom.setAttribute('aria-label', `编辑公式 ${node.attrs.latex.trim()}`);
    drawMath(dom, node.attrs);
    const open = () => onFormula(node, getPos());
    dom.addEventListener('click', open);
    dom.addEventListener('keydown', (e) => {
      if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); open(); }
    });
    return { dom, ignoreMutation: () => true, stopEvent: (e) => e.type === 'click' };
  };
  const rules = inputRules({ rules: [
    wrappingInputRule(/^\s*>\s$/, schema.nodes.blockquote),
    wrappingInputRule(/^\s*([-+*])\s$/, schema.nodes.bullet_list),
    wrappingInputRule(/^(\d+)\.\s$/, schema.nodes.ordered_list, (m) => ({ order: +m[1] })),
    textblockTypeInputRule(/^(#{1,6})\s$/, schema.nodes.heading, (m) => ({ level: m[1].length })),
    textblockTypeInputRule(/^```([\w-]*)\s$/, schema.nodes.code_block, (m) => ({ params: m[1] })),
  ] });
  const plugins = [rules, history(), liveMath(), keymap({
    'Mod-z': undo, 'Mod-y': redo, 'Mod-Shift-z': redo,
    'Mod-b': toggleMark(schema.marks.strong), 'Mod-i': toggleMark(schema.marks.em),
    Enter: splitListItem(schema.nodes.list_item),
    Tab: chainCommands(goToNextCell(1), sinkListItem(schema.nodes.list_item)),
    'Shift-Tab': chainCommands(goToNextCell(-1), liftListItem(schema.nodes.list_item)),
    'Mod-Enter': chainCommands(exitCode, (state, dispatch) => {
      const end = state.doc.content.size;
      const tr = state.tr.insert(end, schema.nodes.paragraph.create());
      dispatch(tr.setSelection(TextSelection.create(tr.doc, end + 1)));
      return true;
    }),
  }), keymap(baseKeymap), tableEditing()];
  const view = new EditorView(host, {
    state: EditorState.create({ schema, doc: parseMarkdown(source), plugins }),
    attributes: { 'aria-label': 'Markdown 正文', spellcheck: 'false', translate: 'no' },
    nodeViews: { math_inline: mathView, math_block: mathView },
    dispatchTransaction(tr) {
      // Composition transactions are left to ProseMirror's IME integration.
      const result = view.state.applyTransaction(tr);
      view.updateState(result.state);
      if (result.transactions.some((t) => t.docChanged)) onChange(toMarkdown(view.state.doc));
    },
    clipboardTextParser(text, context) {
      if (context.parent.type.spec.code) return new Slice(Fragment.from(text ? schema.text(text) : null), 0, 0);
      return Slice.maxOpen(parseMarkdown(text).content);
    },
    clipboardTextSerializer(slice) { return toMarkdown(schema.nodes.doc.create(null, slice.content)); },
    handleDOMEvents: {
      click(view, event) {
        const anchor = event.target.closest('a');
        if (anchor && view.editable) { event.preventDefault(); return true; }
        return false;
      },
    },
  });
  return {
    view,
    load(source) { view.updateState(EditorState.create({ schema, doc: parseMarkdown(source), plugins })); },
    source() { return toMarkdown(view.state.doc); },
    command(name) {
      const commands = {
        bold: toggleMark(schema.marks.strong), italic: toggleMark(schema.marks.em),
        heading: setBlockType(schema.nodes.heading, { level: 2 }),
        list: wrapInList(schema.nodes.bullet_list), code: setBlockType(schema.nodes.code_block), undo, redo,
      };
      commands[name]?.(view.state, view.dispatch, view); view.focus();
    },
    setFormula(pos, attrs, block) {
      let tr = view.state.tr;
      if (pos !== null) tr = tr.setNodeMarkup(pos, undefined, attrs);
      else {
        const node = (block ? schema.nodes.math_block : schema.nodes.math_inline).create(attrs);
        tr = tr.replaceSelectionWith(node);
        if (block && tr.selection instanceof NodeSelection) {
          const end = tr.selection.to;
          tr = tr.insert(end, schema.nodes.paragraph.create());
          tr.setSelection(TextSelection.create(tr.doc, end + 1));
        }
      }
      view.dispatch(tr.scrollIntoView()); view.focus();
    },
  };
}
