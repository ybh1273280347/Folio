# Folio `pi-host` 扩展开发指南

`resources/pi-host` 是 Folio 桌面端的 Node.js AI 运行时宿主（基于 `@earendil-works/pi-coding-agent`），通过 JSONL over stdio 与 Rust 侧通信。

---

## 1. 目录结构与打包规则

- 运行时入口：[host.mjs](host.mjs)
- 通道桥接：[workspace-tool-bridge.mjs](workspace-tool-bridge.mjs)
- 内置自定义工具：
  - [annotation-tools.mjs](annotation-tools.mjs)（`annotation` 通道）
  - [compile-tools.mjs](compile-tools.mjs)（`compile` 通道）
  - [drawio-tools.mjs](drawio-tools.mjs)（`drawio` 通道）
  - [tag-tools.mjs](tag-tools.mjs)（`tag` 通道）
  - [web-tools.mjs](web-tools.mjs) / [python-tool.mjs](python-tool.mjs)（本地执行，不走 Rust 桥接）
- 开发测试：`test/`（仅测试用，不进入发行包）
- 打包脚本：[../../scripts/package.ps1](../../scripts/package.ps1)

> [!IMPORTANT]
> `package.ps1` 使用 `robocopy /E` 全量同步 `resources/pi-host` 到发行包的 `pi-host/`，仅排除 `test`、`__pycache__`、`.pytest_cache` 与 `*.log`/`*.tmp`。
> - 新增运行时 `.mjs` 模块或子目录会自动打包，无需修改 `package.ps1`。
> - 不要把本机调试文件、大体积临时数据放到 `resources/pi-host` 根目录下。

Draw.io 工具提供文件级 `folio_drawio_read`、`folio_drawio_create`、`folio_drawio_update`，以及当前画布级 `folio_drawio_get_selection` 和 `folio_drawio_edit_cells`。画布级工具操作当前活动的 `.drawio` 页签。修改支持新增顶点/连线、按 ID 更新文本/样式/几何/端点，以及按 ID 删除。文件级工具只接受项目内 `.drawio` 相对路径，创建不会覆盖已有文件，写入内容必须是根元素为 `mxfile` 的有效 XML。

---

## 2. 新增工具的两种方式

### 方式 A：纯 JS / 本地子进程工具
适用于不需要读写 Folio 内存状态（编辑器未保存缓冲、项目配置、文件树、标签库等）的工具。
1. 在 `resources/pi-host/` 新增 `<name>-tool.mjs`，按 `@sinclair/typebox` 定义参数。
2. 在 `host.mjs` 的 `createAgentSession({ customTools: [...] })` 中展开注册。
3. 若需命令审批弹窗，在 `host.mjs` 的 `tool_call` 钩子中处理确认逻辑。

### 方式 B：桥接到 Rust `Workspace` 的工具（推荐模式）
适用于需要复用 Rust 原生能力（如 `latexmk` 编译、标签存储、批注持久化、静默保存未落盘缓冲）的工具。

优先在已有功能的通道中增加操作。新增独立能力时，按以下步骤接入。

1. **JS 工具模块**（如 `compile-tools.mjs`）：
   - 导出工厂函数 `(request) => [...]`，调用 `await request('<operation>', params, signal)`。
   - 参照同类工具将结构化结果转换为 Pi 的工具结果，保留错误码和取消状态。
2. **注册 JS 通道**（[host.mjs](host.mjs)）：
   - 在 `workspaceToolChannels` 中按已有对象格式登记 `channel`、`factory` 和适用的超时策略。
   - 宿主创建对应的 `WorkspaceToolBridge`，注册工具并按 `channel` 路由响应。会话释放时，宿主清理该会话的等待请求。
3. **Rust RPC 协议**（[rpc.rs](../../src/ai/rpc.rs) 与 [manager.rs](../../src/ai/manager.rs)）
   - 各通道复用 `workspace_tool_request`、`workspace_tool_cancel` 与 `workspace_tool_response`。通道、请求 ID 和会话归属由现有 RPC 与宿主处理。
4. **Rust Workspace 接入**（[workspace/tools/](../../src/workspace/tools/) 与 [workspace.rs](../../src/workspace.rs)）
   - 在 `AiPaneEvent::WorkspaceTool` 按 `channel` 分派。工具接入代码取得当前项目或文档、维护当前用例的取消状态，再直接调用功能模块。
   - 如需友好工具卡片标题，在 [../../src/ai/mod.rs](../../src/ai/mod.rs) 的 `summarize_tool_call` 与 [../../src/i18n/ai.rs](../../src/i18n/ai.rs) 中补充文案。

---

## 3. 常见踩坑与约束

1. **`stdout` 是纯 JSONL 协议通道**
   - 禁止向 `stdout` 打印调试文本；`host.mjs` 已将 `console.log` 重定向到 `console.error`，第三方模块或子进程也不得污染 `stdout`。
2. **开发态与打包态的路径差异**
   - `cargo run` 读取仓库中的 `resources/pi-host/host.mjs`；打包版 `dist/Folio/Folio.exe` 读取可执行文件同级 `pi-host/host.mjs`。验证桌面打包版前务必重新运行打包脚本或同步 `dist/Folio/pi-host/`。
3. **Rust 工具模块按需导入**
   - `src/workspace.rs` 引入了 GPUI 的宏。工具子模块显式导入需要的父模块类型，避免把 GPUI 测试宏带入普通 Rust 测试。
4. **不要切走前台标签页或占用全局 `Workspace::build`**
   - AI 后台工具应拥有独立的取消句柄（如 `Arc<AtomicBool>`）与隔离的工作目录，不应调用 `open_file` 切换用户当前视图或锁死手动编译。
   - 读写工作区文件前，静默检查 `self.documents.open` 中的 `buffer.deleted` / `buffer.conflict` 与 `is_dirty()`，遇到冲突直接返回结构化错误而非弹出阻塞窗口。
5. **项目路径与权限边界**
   - 核对 `request.project_key` 与当前活动项目一致，沿用能力已有的项目路径校验。创建新文件时校验其父目录及目标文件名。

---

## 4. 验证命令

```pwsh
# JS 宿主、工具桥接与协议端到端测试
npm --prefix resources/pi-host test

# Rust 二进制单元测试（排除集成/耗时 UI 用例）
cargo test --bin folio-gpui-prototype

# 全量编译与静态检查（含测试目标）
cargo check --bin folio-gpui-prototype --tests
```
