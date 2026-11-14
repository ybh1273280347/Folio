import { test, expect } from '@playwright/test';

test('phone: import, render, edit formula, export and recover draft', async ({ page }) => {
  const errors = [];
  page.on('pageerror', (e) => errors.push(e.message));
  await page.addInitScript(() => {
    window.FolioNative = { saveDocument: (name, markdown) => { window.exported = { name, markdown }; } };
  });
  await page.goto('/');
  await page.evaluate(() => window.folioOpenDocument({ name: '公式兼容.md', markdown: String.raw`# 测试

行内 $ x^2 $ 与 \( \frac{1}{2} \)。

\[
\int_0^1 x^2 dx
\]

$$ y = x + 1 $$` + '\n\n`$ code $`' }));
  await expect(page.locator('.formula .katex')).toHaveCount(4);
  await expect(page.locator('.ProseMirror code')).toHaveText('$ code $');
  await page.locator('.formula').first().click();
  await expect(page.getByLabel('LaTeX 源码')).toHaveValue(' x^2 ');
  await page.getByLabel('LaTeX 源码').fill(' x^3 + 1 ');
  await page.getByRole('button', { name: '应用公式' }).click();
  await page.getByRole('button', { name: '切换源码模式' }).click();
  const source = await page.getByLabel('Markdown 源码').inputValue();
  expect(source).toContain('$ x^3 + 1 $');
  expect(source).toContain(String.raw`\( \frac{1}{2} \)`);
  await page.getByRole('button', { name: '导出', exact: true }).click();
  await page.getByRole('button', { name: '保存 Markdown 文件' }).click();
  expect(await page.evaluate(() => window.exported)).toEqual({ name: '公式兼容.md', markdown: source });
  await page.reload();
  // The imported record is at the front of the persisted notebook list.
  await expect(page.getByLabel('笔记名称')).toHaveValue('公式兼容');
  await expect(page.locator('.formula .katex')).toHaveCount(4);
  expect(errors).toEqual([]);
  await page.screenshot({ path: 'test-results/folio-phone.png', fullPage: true });
});

test('live typing, plain Markdown paste, undo and continued text input', async ({ page }) => {
  await page.goto('/');
  await page.getByRole('button', { name: '打开笔记列表' }).click();
  await page.getByRole('button', { name: '新建笔记', exact: true }).click();
  const content = page.locator('.ProseMirror');
  await content.click();
  await page.keyboard.type('$ x $');
  await expect(page.locator('.formula .katex')).toHaveCount(1);
  await page.keyboard.type(' after');
  await expect(content).toContainText('after');
  await page.keyboard.press('Control+z');
  await page.keyboard.press('Control+Shift+z');
  await content.evaluate((element) => {
    const data = new DataTransfer();
    data.setData('text/plain', String.raw`\[ \frac{a}{b} \]`);
    element.dispatchEvent(new ClipboardEvent('paste', { clipboardData: data, bubbles: true, cancelable: true }));
  });
  await expect(page.locator('.formula .katex')).toHaveCount(2);
  await page.getByRole('button', { name: '插入数学公式' }).click();
  await page.getByLabel('LaTeX 源码').fill(String.raw`\sqrt{2}`);
  await page.getByRole('button', { name: '应用公式' }).click();
  await expect(page.locator('.formula .katex')).toHaveCount(3);
  await page.keyboard.type('next line');
  await expect(content).toContainText('next line');
});

test('320px and dark phone layout keep controls and math in bounds', async ({ page }) => {
  await page.setViewportSize({ width: 320, height: 640 });
  await page.emulateMedia({ colorScheme: 'dark' });
  await page.goto('/');
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBe(320);
  const buttons = await page.locator('header button, .toolbar button').evaluateAll((items) => items.map((e) => {
    const r = e.getBoundingClientRect(); return { x: r.x, right: r.right, width: r.width, height: r.height };
  }));
  for (const button of buttons) {
    expect(button.x).toBeGreaterThanOrEqual(0); expect(button.right).toBeLessThanOrEqual(320);
    expect(button.width).toBeGreaterThanOrEqual(44); expect(button.height).toBeGreaterThanOrEqual(44);
  }
  await page.getByRole('button', { name: '插入数学公式' }).click();
  await expect(page.getByLabel('LaTeX 源码')).toBeVisible();
  await page.screenshot({ path: 'test-results/folio-phone-dark.png', fullPage: true });
});

test('multiline display math typed with Enter becomes an editable formula block', async ({ page }) => {
  await page.goto('/');
  await page.getByRole('button', { name: '打开笔记列表' }).click();
  await page.getByRole('button', { name: '新建笔记', exact: true }).click();
  await page.locator('.ProseMirror').click();
  await page.keyboard.type(String.raw`\[`);
  await page.keyboard.press('Enter');
  await page.keyboard.type(String.raw`\frac{1}{2}`);
  await page.keyboard.press('Enter');
  await page.keyboard.type(String.raw`\]`);
  await expect(page.locator('.formula-block .katex')).toHaveCount(1);
  await page.keyboard.type('公式后继续输入');
  await expect(page.locator('.ProseMirror')).toContainText('公式后继续输入');
  await page.locator('.formula-block').click();
  await expect(page.getByLabel('LaTeX 源码')).toHaveValue('\n\\frac{1}{2}\n');
});

test('editing imported escaped prices keeps them literal', async ({ page }) => {
  await page.goto('/');
  await page.evaluate(() => window.folioOpenDocument({ name: '价格.md', markdown: String.raw`价格 \$5 和 \$10。` }));
  await page.locator('.ProseMirror').click();
  await page.keyboard.press('Control+End');
  await page.keyboard.type(' added');
  await expect(page.locator('.formula')).toHaveCount(0);
  await page.getByRole('button', { name: '切换源码模式' }).click();
  expect(await page.getByLabel('Markdown 源码').inputValue()).toContain(String.raw`\$5 和 \$10`);
});

test('Honor GT Pro: high density portrait, landscape and reduced keyboard viewport', async ({ browser }) => {
  // Physical resolution does not define CSS density. Cover both common display
  // scales; the 2x context produces an exact 1264 x 2800 screenshot.
  const context = await browser.newContext({ viewport: { width: 632, height: 1400 }, deviceScaleFactor: 2, isMobile: true, hasTouch: true });
  const page = await context.newPage();
  await page.goto('http://127.0.0.1:5190/');
  await expect(page.locator('.formula .katex')).toHaveCount(4);
  await page.screenshot({ path: 'test-results/folio-honor-1264x2800.png' });
  for (const viewport of [{ width: 421, height: 933 }, { width: 933, height: 421 }, { width: 421, height: 430 }]) {
    await page.setViewportSize(viewport);
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBe(viewport.width);
    await expect(page.getByRole('button', { name: '导出', exact: true })).toBeVisible();
    await page.getByRole('button', { name: '插入数学公式' }).click();
    await page.getByLabel('LaTeX 源码').fill(String.raw`\frac{a+b}{c+d}`);
    await page.getByRole('button', { name: '应用公式' }).click();
    await expect(page.locator('#formula-dialog')).not.toBeVisible();
  }
  await context.close();
});
