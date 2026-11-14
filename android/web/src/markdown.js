/** Markdown is the persisted document; rendered math is a disposable projection.
 * Recognize TeX before Markdown consumes backslash escapes, like desktop Folio.
 */
import MarkdownIt from 'markdown-it';
import { Schema } from 'prosemirror-model';
import { tableNodes } from 'prosemirror-tables';
import { schema as baseSchema, defaultMarkdownParser, defaultMarkdownSerializer, MarkdownParser, MarkdownSerializer } from 'prosemirror-markdown';

const fences = [
  ['$$', '$$', true], ['\\\\[', '\\\\]', true], ['\\\\(', '\\\\)', false],
  ['\\[', '\\]', true], ['\\(', '\\)', false], ['$', '$', false],
];

function escaped(source, index) {
  let slashes = 0;
  while (index > 0 && source[--index] === '\\') slashes++;
  return slashes % 2 === 1;
}

/** Offsets are JS UTF-16 positions, also used by ProseMirror text nodes.
 * Spaces inside fences are intentional math, including `$ x $`.
 */
export function mathAt(source, start) {
  if (escaped(source, start)) return null;
  const fence = fences.find(([open]) => source.startsWith(open, start));
  if (!fence) return null;
  const [open, close, display] = fence;
  const bodyStart = start + open.length;
  let end = source.indexOf(close, bodyStart);
  while (end >= 0) {
    // A single-dollar fence never consumes half of a double-dollar fence.
    if (!escaped(source, end) && !(open === '$' && (source[end - 1] === '$' || source[end + 1] === '$'))) break;
    end = source.indexOf(close, end + close.length);
  }
  if (end < 0) return null;
  const latex = source.slice(bodyStart, end);
  if (!latex.trim() || (!display && latex.includes('\n'))) return null;
  return { start, end: end + close.length, latex, open, close, display };
}

/** Only the rendering projection unescapes an explicitly copied JSON fence.
 * Ordinary aligned/matrix `\\` row separators stay untouched.
 */
export function renderLatex(attrs) {
  return (attrs.open.startsWith('\\\\') ? attrs.latex.replaceAll('\\\\', '\\') : attrs.latex).trim();
}

function mathPlugin(md) {
  md.inline.ruler.before('escape', 'folio_math', (state, silent) => {
    const match = mathAt(state.src, state.pos);
    if (!match) return false;
    if (!silent) {
      const token = state.push('math_inline', '', 0);
      token.meta = match;
    }
    state.pos = match.end;
    return true;
  });
  md.block.ruler.before('fence', 'folio_math_block', (state, startLine, endLine, silent) => {
    if (state.sCount[startLine] - state.blkIndent >= 4) return false;
    const start = state.bMarks[startLine] + state.tShift[startLine];
    const match = mathAt(state.src, start);
    if (!match?.display) return false;
    let last = startLine;
    while (last < endLine && state.eMarks[last] < match.end) last++;
    if (last >= endLine || state.src.slice(match.end, state.eMarks[last]).trim()) return false;
    if (silent) return true;
    const token = state.push('math_block', '', 0);
    token.block = true;
    token.meta = match;
    token.map = [startLine, last + 1];
    state.line = last + 1;
    return true;
  }, { alt: ['paragraph', 'reference', 'blockquote', 'list'] });
}

const attrs = {
  latex: { default: '' }, open: { default: '$' }, close: { default: '$' }, display: { default: false },
};
const mathSpec = (block) => ({
  group: block ? 'block' : 'inline', inline: !block, atom: true, selectable: true,
  attrs,
  toDOM(node) { return [block ? 'div' : 'span', { 'data-math': '', class: 'math-source' }, `${node.attrs.open}${node.attrs.latex}${node.attrs.close}`]; },
});

export const schema = new Schema({
  nodes: baseSchema.spec.nodes.append({ math_inline: mathSpec(false), math_block: mathSpec(true),
    ...tableNodes({ tableGroup: 'block', cellContent: 'block+' }),
  }),
  marks: baseSchema.spec.marks,
});
export const tokenizer = new MarkdownIt('commonmark', { html: false, breaks: false }).enable('table').use(mathPlugin);
tokenizer.core.ruler.after('block', 'folio_table_paragraphs', (state) => {
  // Markdown-it table cells contain inline tokens. ProseMirror cells own normal
  // paragraphs so editing, selection and math share the rest of the document.
  const tokens = [];
  for (const token of state.tokens) {
    if (token.type === 'td_close' || token.type === 'th_close') tokens.push(new state.Token('paragraph_close', 'p', -1));
    tokens.push(token);
    if (token.type === 'td_open' || token.type === 'th_open') tokens.push(new state.Token('paragraph_open', 'p', 1));
  }
  state.tokens = tokens;
});
export const parser = new MarkdownParser(schema, tokenizer, {
  ...defaultMarkdownParser.tokens,
  math_inline: { node: 'math_inline', getAttrs: (token) => token.meta },
  math_block: { node: 'math_block', getAttrs: (token) => token.meta },
  table: { block: 'table' }, tr: { block: 'table_row' },
  th: { block: 'table_header' }, td: { block: 'table_cell' },
  thead: { ignore: true }, tbody: { ignore: true },
});
export const serializer = new MarkdownSerializer({
  ...defaultMarkdownSerializer.nodes,
  math_inline(state, node) { state.write(`${node.attrs.open}${node.attrs.latex}${node.attrs.close}`); },
  math_block(state, node) {
    state.write(`${node.attrs.open}${node.attrs.latex}${node.attrs.close}`);
    state.closeBlock(node);
  },
  table(state, node) {
    node.forEach((row, _, rowIndex) => {
      const cells = [];
      row.forEach((cell) => cells.push(serializer.serialize(cell).replaceAll('\n', ' ').replaceAll('|', '\\|')));
      state.write(`| ${cells.join(' | ')} |\n`);
      if (rowIndex === 0) state.write(`| ${cells.map(() => '---').join(' | ')} |\n`);
    });
    state.closeBlock(node);
  },
}, defaultMarkdownSerializer.marks, { escapeExtraCharacters: /\$/g });

export const parseMarkdown = (source) => parser.parse(source);
export const toMarkdown = (doc) => serializer.serialize(doc);
