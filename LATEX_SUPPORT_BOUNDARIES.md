# LaTeX 数学语法当前支持边界

本文档记录当前项目中 `vendor/latex-rust`（含本地扩展）**已支持**与**明确不支持**的语法边界，用于快速排查公式报错是「公式写法有误」还是「引擎尚未支持、需要拓展」。

> **排查速查**：
> - 引擎原则：**不支持的语法一律直接报错**（`Unknown` / `Unsupported` / `Malformed`），不会静默降级或伪造渲染。
> - 如果报错信息包含 `unknown command` / `unsupported environment` / `unsupported ...`，先对照下表确认是否在当前白名单内；若在支持范围内但报 `malformed` / `unmatched`，则是公式写法问题（如括号/环境未闭合、缺少参数、在 Markdown 中未用 `$...$` / `$$...$$` 包裹等）。

---

## 1. 定界符与嵌套限制

* **公式定界符**：支持 `$...$`、`$$...$$`、`\(...\)`、`\[...\]`（在 Markdown 中需使用 `$...$` 或 `$$...$$` 才能被识别为公式节点）。
* **最大嵌套深度**：默认 `DEFAULT_MAX_NESTING_DEPTH = 32`（例如 `\frac{1}{...}` 最多嵌套 15 层左右）；超出返回 `Malformed("input nests deeper than 32 levels")`。

---

## 2. 环境（`\begin{...}...\end{...}`）

### 已支持的环境
| 类别 | 环境名 | 说明 |
| :--- | :--- | :--- |
| 矩阵类 | `matrix`、`pmatrix`、`bmatrix`、`vmatrix`、`Vmatrix`、`Bmatrix` | 居中对齐，分别对应无边框 / `()` / `[]` / `\| \|` / `‖ ‖` / `{}` |
| 分段与表格 | `cases`、`array` | `array` 仅支持列格式 `l`、`c`、`r`、`\|`（不支持 `@`、`!`、`>`、`<`、`p`、`m`、`b`、`*`） |
| 对齐与方程（编号） | `align`、`gather`、`multline`、`equation` | Display 样式；支持 `\tag{...}`、`\tag*{...}`、`\nonumber`、`\notag`、`\label{...}`、`\ref{...}` |
| 对齐与方程（无编号） | `align*`、`gather*`、`multline*`、`equation*`（本地扩展）、`aligned`、`gathered`、`split` | 其中 `align*` / `gather*` / `multline*` / `equation*` 为本地扩展 |
| 环境内辅助命令 | `\hline`、`\\`、`\cr`、`\intertext{...}` | 注意：`align` / `gather` 内不支持 `\hline` |

### 明确不支持的常见环境
* `alignat` / `alignat*`、`flalign` / `flalign*`、`xalignat`、`xxalignat`
* `smallmatrix`、`rcases`、`dcases`、`alignedat`、`subarray`、`eqnarray`
* `array` 中的高级列格式（如 `p{...}`、`@{...}`）

---

## 3. 结构、分式、根式与上下标

* **分式与二项式**：`\frac`、`\dfrac`、`\tfrac`、`\cfrac`、`\binom`、`\dbinom`、`\tbinom`、`\genfrac{l}{r}{thickness}{style}{num}{den}`、中缀 `{a \over b}`、中缀 `{n \choose k}`
* **根式**：`\sqrt{...}`、`\sqrt[n]{...}`
* **上下标与堆叠**：`_`、`^`、`\overset`、`\stackrel`、`\underset`、`\substack{... \\ ...}`
* **样式切换指令**：`\displaystyle`、`\textstyle`、`\scriptstyle`、`\scriptscriptstyle`、`\limits`、`\nolimits`

---

## 4. 括号与定界符

* **自适应大小**：`\left ... \right`（不支持 `\middle` 的独立节点解析，仅在括号扫描跳过）
* **手动大小**：`\big`、`\Big`、`\bigg`、`\Bigg` 及 `l` / `r` / `m` 后缀（如 `\bigl`、`\Bigr`、`\biggm`）
* **可用定界符**：
  * 字符：`(`、`)`、`[`、`]`、`|`、`/`、`<`、`>`、`.`（空定界符）
  * 命名：`\{`、`\}`、`\|`、`\langle`、`\rangle`、`\lfloor`、`\rfloor`、`\lceil`、`\rceil`、`\lvert`、`\rvert`、`\lVert`、`\rVert`、`\vert`、`\Vert`、`\uparrow`、`\downarrow`、`\Uparrow`、`\Downarrow`、`\updownarrow`、`\Updownarrow`、`\backslash`、`\lgroup`、`\rgroup`、`\lmoustache`、`\rmoustache`

---

## 5. 运算符、大算子与取模

* **大算子**：`\sum`、`\prod`、`\int`、`\iint`、`\iiint`、`\oint`、`\oiint`、`\lim`、`\coprod`、`\bigcup`、`\bigcap`、`\bigsqcup`、`\bigvee`、`\bigwedge`、`\bigoplus`、`\bigotimes`、`\biguplus`
* **标准函数名**：`\sin`、`\cos`、`\tan`、`\cot`、`\sec`、`\csc`、`\arcsin`、`\arccos`、`\arctan`、`\sinh`、`\cosh`、`\tanh`、`\coth`、`\log`、`\ln`、`\lg`、`\exp`、`\limsup`、`\liminf`、`\sup`、`\inf`、`\max`、`\min`、`\det`、`\dim`、`\ker`、`\deg`、`\gcd`、`\lcm`、`\Pr`、`\arg`
* **自定义算子**：`\operatorname{...}`（不支持带星号 `\operatorname*{...}`）
* **取模命令（本地扩展）**：`\pmod{...}`、`\pod{...}`、`\mod{...}`、`\bmod`

---

## 6. 重音、修饰与箭头

* **上/下重音**：`\hat`、`\check`、`\breve`、`\acute`、`\grave`、`\tilde`、`\bar`、`\vec`、`\dot`、`\ddot`、`\dddot`、`\ddddot`、`\widehat`、`\widetilde`、`\mathring`、`\overline`、`\underline`、`\overbrace`、`\underbrace`、`\overleftarrow`、`\overrightarrow`、`\overleftrightarrow`、`\underleftarrow`、`\underrightarrow`、`\underleftrightarrow`
* **删除线与边框**：`\cancel`、`\bcancel`、`\xcancel`、`\cancelto{val}{expr}`、`\boxed`、`\fbox`、`\not`
* **可伸长箭头**：`\xrightarrow[under]{over}`、`\xleftarrow[under]{over}`
  * **暂不支持**：`\xRightarrow`、`\xLeftarrow`、`\xleftrightarrow`、`\xhookrightarrow` 等其他扩展箭头

---

## 7. 字体、文本、颜色与间距

* **数学字体**：`\mathrm` / `\textrm` / `{\rm ...}`、`\mathbf` / `\textbf` / `{\bf ...}`、`\mathit` / `\textit` / `{\it ...}`、`\mathsf` / `\textsf` / `{\sf ...}`、`\mathtt` / `\texttt` / `{\tt ...}`、`\mathbb`、`\mathcal` / `{\cal ...}`、`\mathfrak`、`\mathscr`、`\boldsymbol`、`\pmb`
  * **注意**：其他 `\math*` 字体命令会直接报 `Unsupported("font style ...")`
* **文本模式**：`\text{...}`、`\mbox{...}`（支持中文字体回退及内部 `$...$` / `\(...\)` 行内数学）
* **颜色**：`\textcolor[model]{spec}{...}`、`\colorbox`、`\fcolorbox`、`\definecolor{name}{model}{spec}`
* **间距与占位**：`\,`、`\:`、`\>`、`\;`、`\!`、`\quad`、`\qquad`、`\ `、`\hspace{dim}`（单位仅支持 `em`、`mu`、`pt`、`bp`）、`\phantom`、`\vphantom`、`\hphantom`、`\strut`、`\rule{w}{h}`

---

## 8. 符号表与常见不支持项

* **符号表**：覆盖 `vendor/latex-rust/data/symbols.tsv` 中的标准符号，以及常见别名（`\le`、`\ge`、`\ne`、`\dots`、`\lnot`、`\dag`、`\ddag`、`\owns`）和补充符号（大写希腊字母、`\varepsilon` 等变体、`\ldots` / `\cdots` / `\vdots` / `\ddots`、`\implies`、`\iff`、`\to`、`\gets` 等）。
* **明确不支持的 LaTeX 通用能力**：
  * 自定义宏定义：`\newcommand`、`\renewcommand`、`\def`、`\DeclareMathOperator`
  * 交叉引用与编号进阶：`\eqref`（目前仅支持 `\ref`）、`\numberwithin`
  * 底层 TeX 原语：`\kern`、`\mkern`、`\raisebox`、`\vcenter`、`\smash`、`\mathllap` / `\mathrlap`
  * 物理/化学扩展包语法：`\ce{...}`（mhchem）、`\bra{...}` / `\ket{...}`（braket）、`\dv` / `\pdv`（physics）
