# 轻量公式排版选型与本机测量

日期：2026-11-11。范围：为 Markdown 编辑器寻找进程内公式排版方案；保留 GPUI 原生字形绘制、现有数学字体选择、中文以及两类公式分隔符。

## 结论

优先选择 `latex-rust 3.0.3` 的解析器和盒模型作为接入候选。它直接接收 LaTeX，能够使用项目已有的三种 OpenType 数学字体，实测排版耗时已经到亚毫秒量级，不需要浏览器、JavaScript 运行时、子进程或 SVG 中转。

2026-11-11 已接入 `latex-rust` 进程内后端：补齐宿主字体回退度量与字形来源，盒模型映射到 GPUI 字形、横线、背景、边框及斜线。数学字体沿用现有资源，不启动 XeTeX，也不生成 SVG。Markdown PDF 直接消费 GPUI 文档绘制操作，使用相同字体、公式布局和正文宽度，并嵌入字体保留文字复制。

## 其他产品使用的引擎

| 产品 | 已核实的实现 |
| --- | --- |
| Notion | KaTeX；官方帮助明确说明其公式渲染使用 KaTeX |
| Typora | MathJax；官方数学功能说明明确标注 |
| Obsidian | 官方 API 提供 `loadMathJax`、`renderMath`，以及刷新 MathJax 样式的 `finishRenderMath` |

这些事实说明可以采用专门的公式排版器；此处没有测量这三个产品的延迟或内存。

来源：[Notion](https://www.notion.com/help/math-equations)、[Typora](https://support.typora.io/Math/)、[Obsidian 官方 API](https://raw.githubusercontent.com/obsidianmd/obsidian-api/master/obsidian.d.ts)。

## 当前路径的成本

替换前的 `src/math_layout.rs` 为新公式写入临时 TeX 文件，启动 XeTeX，加载 `standalone`、`amsmath`、`unicode-math`、`xeCJK` 和字体，读取 XDV 后转换为字形坐标。后台预热和缓存减少再次打开时的等待，但没有移除新公式首次排版的进程启动和宏包加载成本。以下测量记录保留用于说明替换依据，耗时来自独立排版进程，不等于完整界面的交互延迟。

## 独立 Release 测量

环境：本机 Windows、Rust GNU 1.99.1、Release `opt-level=2`。表达式取自前面使用过的琴生不等式：

```latex
\boxed{f\left(\frac{a+b}{2}\right)\leq\frac{f(a)+f(b)}2}
```

| 指标 | 当前 XeTeX 方式 | latex-rust 3.0.3 |
| --- | ---: | ---: |
| 首次操作 | 5668.32 ms | 1.2503 ms |
| 重复解析与排版 1000 次 | 未重复执行完整 TeX 编译 | 198.355 ms，总计约 0.198 ms/条 |
| 独立进程观察到的峰值工作集 | 211.81 MiB | 13.71 MiB |

测量边界：

- XeTeX 包括启动子进程、加载与应用相同的宏包/字体、写出 XDV；没有计算后续 XDV 解析和 GPUI 绘制。
- Rust 首次操作包括构造字体、解析表达式与生成盒模型；不包括启动独立测量程序。目标应用会把库直接嵌入已有进程，因此不会为每个公式启动程序。
- Rust 重复测量每次都重新解析和排版，没有使用公式结果缓存。
- 工作集来自 Windows `PeakWorkingSet64`。独立程序同时链接了两个候选库，在不同进程中分别运行各自分支；这些数字不是完整 Folio 的内存，也不是接入后的内存增量。
- 这是一次本机排版性能测量，没有运行自动化测试套件或界面自动操作，也没有进行视觉验收。

## 公式与字体覆盖观察

`latex-rust` 对以下 12 个非中文样例均能生成布局：嵌套 `\overline`、基数等式、分式、`\boxed` 琴生不等式、带次数根号、集合与 `\mathbb`、`\operatorname`、求和上下限、矩阵、`aligned`、`cases`、组合重音/上括号。

所观察的单条非中文样例耗时约为 0.088–0.521 ms。STIX Two Math、Latin Modern Math、TeX Gyre Pagella Math 均通过 `MathFont::from_bytes` 加载并完成排版。

两个中文样例 `f(\text{平均})\leq\text{平均}(f)` 和 `平均值=\frac{a+b}{2}` 都明确返回 `missing glyph for '平'`。库目前不会选择中文字体，不能直接把这个版本作为完整替换发布。

## 未选用 math-core + formulary 的原因

组合版本为 `math-core 0.9.0 + formulary 0.3.0`，只启用 formulary 的 `shaping`，没有启用 SVG。它的纯布局更快，独立进程峰值工作集约 6.46 MiB，但观察到现有需求的实质性缺口：

1. `\boxed` 转成 `menclose`，formulary 报 `UnknownElement { element: "menclose" }`，方框不会被保留。
2. `\operatorname{card}(A)` 的中间输出含 `&ApplyFunction;`，XML 解析报 `UnknownEntityReference`。
3. 对数学字体里不存在的中文，布局代码会跳过字形。输出没有报错、也没有保留 glyph id 0，但中文没有被绘制。不能把没有异常解释为中文渲染成功。

这些缺口比直接 LaTeX 盒模型方案需要更多适配工作，因此本轮不优先选择它。

## 接入边界

已采用的应用路径：

```text
现有分隔符识别
    → LaTeX AST
    → 进程内数学盒模型（复用字体对象）
    → GPUI 字形 / 横线 / 方框 / 颜色等绘制原语
```

- 只启用 `latex-rust` 的核心 `std` 功能，不启用它的 PNG、egui 后端，不增加 GUI 或 JavaScript 运行时。
- 使用项目已有字体字节构造字体，避免重复引入内置 STIX 字体数据。
- 字体对象共享；公式缓存只存解析/排版结果，字号和 DPI 变化继续由 GPUI 绘制。
- 中文字体选择必须发生在布局阶段，输出需要记录字形所属字体。不能用数学字体的度量排版后，仅在绘制时换成中文字形。
- GPUI 适配器必须完整处理实际返回的盒模型内容，尤其是方框、装饰线、基线和脚本缩放。
- 不支持的输入明确报错，不通过静默回退启动 XeTeX 来掩盖延迟。
- 整篇 `.tex` 的 PDF 编译继续由现有 XeTeX / latexmk 流程承担。

官方接口来源：[latex-rust 3.0.3](https://docs.rs/latex-rust/3.0.3/latex_rust/)、[formulary 0.3.0](https://docs.rs/formulary/0.3.0/formulary/)、[math-core 0.9.0](https://docs.rs/math-core/0.9.0/math_core/)。

## 复现材料

独立程序与日志保存在：

```text
C:/Users/12732/AppData/Local/Temp/folio-math-evaluation/
    Cargo.toml
    Cargo.lock
    src/main.rs
    xetex-current.tex
    xetex-stdout.txt
    xetex-stderr.txt
    latex-rust-results.txt
    latex-rust-errors.txt
    formulary-results.txt
    formulary-errors.txt
```

构建和运行候选库：

```powershell
$env:CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER = (Get-Command gcc).Source
cargo build --release --manifest-path C:/Users/12732/AppData/Local/Temp/folio-math-evaluation/Cargo.toml
C:/Users/12732/AppData/Local/Temp/folio-math-evaluation/target/release/folio-math-evaluation.exe latex-rust
C:/Users/12732/AppData/Local/Temp/folio-math-evaluation/target/release/folio-math-evaluation.exe formulary
```
