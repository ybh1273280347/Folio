import test from 'node:test';
import assert from 'node:assert/strict';
import katex from 'katex';
import { parseMarkdown, toMarkdown, renderLatex } from '../src/markdown.js';

function formulas(doc) {
  const nodes = [];
  doc.descendants((node) => { if (node.type.name.startsWith('math_')) nodes.push(node); });
  return nodes;
}

test('all four fences accept inner spaces and render real TeX', () => {
  const source = String.raw`中文 $ x^2 $ 与 \( \frac{1}{2} \)。

$$ \sum_{k=1}^{n} k $$

\[
 \begin{aligned}a&=1\\b&=2\end{aligned}
\]`;
  const nodes = formulas(parseMarkdown(source));
  assert.equal(nodes.length, 4);
  assert.deepEqual(nodes.map((n) => n.attrs.display), [false, false, true, true]);
  assert.equal(nodes[0].attrs.latex, ' x^2 ');
  for (const node of nodes) assert.match(katex.renderToString(renderLatex(node.attrs), { displayMode: node.attrs.display }), /class="katex"/);
  const saved = toMarkdown(parseMarkdown(source));
  assert.match(saved, /\$ x\^2 \$/);
  assert.ok(saved.includes(String.raw`\( \frac{1}{2} \)`));
  assert.deepEqual(formulas(parseMarkdown(saved)).map((n) => n.attrs), nodes.map((n) => n.attrs));
});

test('code, escaped currency and incomplete fences stay text', () => {
  const source = '价格 \\$20，未闭合 $ x。\n\n`$ a $` 与 `\\(b\\)`\n\n```tex\n$$ c $$\n\\[ d \\]\n```';
  const doc = parseMarkdown(source);
  assert.equal(formulas(doc).length, 0);
  assert.ok(doc.textContent.includes('$$ c $$'));
  assert.ok(toMarkdown(doc).includes('`$ a $`'));
});

test('formula bodies protect Markdown characters and escaped dollar commands', () => {
  const source = String.raw`公式 $ a_b * c + \text{cost: \$5} $ 与 \[ x_1 * x_2 \]。`;
  const nodes = formulas(parseMarkdown(source));
  assert.equal(nodes.length, 2);
  assert.ok(nodes[0].attrs.latex.includes(String.raw`\$5`));
  assert.ok(nodes[1].attrs.display);
  assert.equal(toMarkdown(parseMarkdown(toMarkdown(parseMarkdown(source)))), toMarkdown(parseMarkdown(source)));
});

test('copied double-backslash fences render while saved source is preserved', () => {
  const source = String.raw`\\( \\frac{1}{2} \\)`;
  const node = formulas(parseMarkdown(source))[0];
  assert.equal(renderLatex(node.attrs), String.raw`\frac{1}{2}`);
  assert.equal(toMarkdown(parseMarkdown(source)), source);
});

test('Markdown source cannot become active HTML', () => {
  const doc = parseMarkdown('<script>alert(1)</script>\n\n[x](javascript:alert(1))');
  assert.ok(doc.textContent.includes('<script>'));
  assert.equal(doc.firstChild.firstChild.marks.length, 0);
});

test('table cells remain editable paragraphs and retain both math syntaxes', () => {
  const source = String.raw`| 变量 | 定义 |
| --- | --- |
| $ x $ | \( \frac{1}{2} \) |`;
  const doc = parseMarkdown(source);
  assert.equal(doc.firstChild.type.name, 'table');
  assert.equal(doc.firstChild.lastChild.firstChild.firstChild.type.name, 'paragraph');
  assert.equal(formulas(doc).length, 2);
  assert.equal(formulas(parseMarkdown(toMarkdown(doc))).length, 2);
  assert.equal(parseMarkdown(toMarkdown(doc)).textContent, doc.textContent);
});

test('escaped dollar amounts stay literal across save and reload', () => {
  const source = String.raw`价格 \$5 和 \$10，保留文字。`;
  const saved = toMarkdown(parseMarkdown(source));
  assert.equal(formulas(parseMarkdown(saved)).length, 0);
  assert.equal(parseMarkdown(saved).textContent, parseMarkdown(source).textContent);
});
