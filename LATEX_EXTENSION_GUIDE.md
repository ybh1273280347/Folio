# LaTeX 数学语法拓展指南

本项目使用 vendored 的纯 Rust LaTeX 数学排版引擎 `vendor/latex-rust`（v3.0.3）。由于引擎采用严格白名单解析，未登记的命令或环境会直接返回 `ParseError::Unknown` / `ParseError::Unsupported`。本指南说明如何在 `vendor/latex-rust` 中按规范拓展新语法。

---

## 1. 整体渲染链路

1. **Markdown 提取**：`src/math.rs` 中的 `MathPlugin` 从 Markdown AST 提取 `Node::Math` / `Node::InlineMath`。
2. **解析与布局**：`src/math_layout.rs` 中的 `Typesetter::typeset` 调用：
   - `latex_rust::parse(latex)` → `MathNode` AST
   - `latex_rust::layout(&ast, &font, style)` → `MathBox` 盒模型
3. **`latex-rust` 内部流水线**：
   - `src/parser/preproc.rs` & `token.rs`：空白裁剪与分词（`Token`）
   - `src/parser/group_rules.rs`：Token 级轻量改写（如 `{\rm x}` → `\mathrm{x}`、`{a \over b}` → `\frac{a}{b}`）
   - `src/parser/parse/drive.rs`：堆栈式状态机解析（命令分发 `classify`、环境分发 `start_begin`）
   - `src/parser/ast.rs`：AST 定义（`MathNode`、`MatrixStyle`、`AccentKind` 等）与 `gold` / `to_latex` 序列化
   - `src/layout/engine_walk.rs` & `engine.rs`：堆栈式盒模型排版
   - `src/layout/numbering.rs`：公式编号与 `\label` / `\ref`

---

## 2. 四类常见拓展方式

### 场景 A：简单命令 / 无参符号
适用：缺失的符号、二元关系、运算符（例如 `\bmod`、常用数学函数名）。

- **已有 Unicode 对应字形**：在 `vendor/latex-rust/src/symbols.rs` 中确认或补充符号表条目，或在 `vendor/latex-rust/src/parser/parse/drive.rs` 的 `classify` 中直接返回 `MathNode::Symbol(...)` / `MathNode::Operator(name, limits)`。
- **可直接由现有节点组合的无参命令**（如 `\bmod`）：在 `drive.rs` 的 `classify` 中返回 `MathNode::Row(...)`（例如 `Space` + `Text(..., TextStyle::Rm)` + `Space`）。

### 场景 B：可等价展开的带参数宏（推荐优先考虑）
适用：`\pmod{...}`、`\pod{...}`、`\mod{...}` 等可以由现有 `MathNode` 原语（`Row`、`Space`、`Atom`、`Text`、`Fraction` 等）精确表达的命令。

**步骤（仅需修改 `vendor/latex-rust/src/parser/parse/drive.rs`）：**
1. 在 `Cmd` 枚举中新增变体（例如 `Mod { kind: ModKind, phase: One }`）。
2. 在 `classify` 中匹配命令名，返回 `Ok(Classified::Cmd(...))`。
3. 在 `step_cmd` 中使用 `one_call(phase, ready, true, Kind::arg)`（单参数）或 `pair(...)`（双参数）读取参数节点，然后组装为 `MathNode::Row` 返回 `Ok(Step::Done(...))`。

> **优势**：无需改动 `MathNode` 枚举、`engine_walk.rs`、`engine.rs` 和 `numbering.rs`，同时天然支持任意嵌套参数（如 `\pmod{\frac{p-1}{2}}`）与深度限制。

### 场景 C：新增环境或环境变体（如 `align*`、`gather*`）
适用：复用现有矩阵/对齐布局逻辑的新环境，或带星号的无编号变体。

**步骤：**
1. **`vendor/latex-rust/src/parser/ast.rs`**：
   - 在 `MatrixStyle` 中新增枚举变体（须附带 `///` 文档注释）。
   - 更新 `MatrixStyle::gold`（返回环境名字符串，用于 `to_latex`）。
   - 按需更新 `MatrixStyle::is_display_env`、`numbers_rows`、`numbers_once`。
2. **`vendor/latex-rust/src/parser/parse/drive.rs`**：
   - 在 `start_begin` 中添加 `"环境名" => MatrixStyle::...` 映射。
3. **`vendor/latex-rust/src/layout/engine_walk.rs`**：
   - 若走多列对齐状态机，更新 `start_align`；在 `plan_matrix` 中补充新变体的分支。
4. **`vendor/latex-rust/src/layout/engine.rs`**：
   - 在 `matrix` 分发中添加新变体，路由到对应的布局方法（注意无编号环境不要调用 `self.take_number()`）。

### 场景 D：全新排版原语（需要自定义 TeX 几何布局）
适用：现有 `MathNode` 无法表达的结构（例如复杂划线、特殊边框或自定义对齐矩阵）。

需要依次打通以下 5 处：
1. `vendor/latex-rust/src/parser/ast.rs`：在 `MathNode` 中新增变体，并补齐 `clone` / `gold` / `to_latex` 的匹配分支。
2. `vendor/latex-rust/src/parser/parse/drive.rs`：在 `Cmd` / `classify` / `step_cmd` 中解析为新 `MathNode`。
3. `vendor/latex-rust/src/layout/numbering.rs`：在 `push_collect` 中补充子节点遍历。
4. `vendor/latex-rust/src/layout/engine_walk.rs`：在 `plan_node` 中规划子节点的 `MathStyle` 调度。
5. `vendor/latex-rust/src/layout/engine.rs`：在 `item_inner` 中实现具体的 `MathBox` 拼装逻辑。

---

## 3. 编码规范与约束

`vendor/latex-rust/src/lib.rs` 启用了以下编译期严格检查，修改时必须遵守：
* **`#![deny(missing_docs)]`**：所有 `pub` 枚举变体、结构体字段和函数必须有 `///` 文档注释。
* **`#![deny(clippy::float_arithmetic)]`**：`latex-rust` 内部禁止使用 `f32`/`f64` 浮点运算，所有尺寸必须使用精确有理数 `Dim`。
* **堆栈遍历**：解析与布局均通过显式 `Vec` 栈驱动以支持 `max_depth`，不要在 `drive.rs` 或 `engine_walk.rs` 中引入无界原生递归。

---

## 4. 修改后如何验证

由于 `vendor/latex-rust` 未加入根工作区且其独立测试依赖（如 `proptest`、`egui`）不在主工程 `Cargo.lock` 中，请通过主工程进行编译与测试验证：

```bash
# 1. 快速检查编译（含 missing_docs 与 float_arithmetic 约束）
cargo check --quiet

# 2. 运行数学排版单元测试（用例位于 src/math_layout.rs）
cargo test --bin folio-gpui-prototype math_layout::tests
```

新增语法后，建议在 `src/math_layout.rs` 的 `tests` 模块中补充对应的 `parse` + `layout` 测试用例。
