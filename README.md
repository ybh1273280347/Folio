# Folio (GPUI)

基于 [GPUI](https://www.gpui.rs/) 与 Rust 编写的 Windows 原生 LaTeX / Markdown 个人工作台，另附一个独立的离线 Android 版。

写文档和论文时，希望有一款轻量、启动快、内存占用低的本地编辑工具，于是用 GPUI 做了这套原生工作台。界面、编辑器、公式排版、PDF 预览与导出均在进程内完成；打开 `.html` 文件时才按需调用系统 WebView2。项目另有一个纯前端的 Android 版本（见 [android/README.md](android/README.md)）。这里是个人分享，供有类似需求或对 GPUI 开发感兴趣的朋友参考。

---

## 💡 设计特点

- **原生渲染为主**：界面、编辑器、数学公式与 PDF 的排版/预览/导出全部基于 GPUI 原生渲染，不打包 Electron/Chromium。唯一的 WebView 用法是打开 `.html/.htm` 时按需调用**系统 WebView2**（Edge Chromium 内核）做交互预览，且惰性创建、关闭即释放。
- **进程内数学公式排版**：Markdown 内嵌的 LaTeX 公式基于 `latex-rust` 盒模型在进程内解析与布局，直接由 GPUI 绘制字形与矢量横线，无需生成 SVG 或拉起 KaTeX / WebView。内置 STIX Two、Latin Modern、TeX Gyre Pagella 三套数学字体，中文缺字回退系统字体。
- **矢量 PDF 导出**：Markdown 导出 PDF 复用文档绘制操作并通过 `krilla` 输出，正文与公式均保留为可选择、可复制的矢量文字，不调用无头浏览器。
- **纯 Rust PDF 预览**：通过 vendored `gpui-pdf`（渲染后端 `hayro`）实现页面虚拟化、按需光栅化、目录导航、缩略图与高亮批注；文本层支持搜索、选择与复制。
- **克制的资源管理**：PDF 仅缓存视口附近页面，缩放或改窗口时丢弃未完成的旧渲染任务，隐藏标签/分屏时释放资源；文件监听（`notify`）只比对当前打开文件，目录结构变动才重扫文件树；编辑器草稿随击键更新，不做定时全量轮询。
- **可配置快捷键**：所有工作区与 Markdown 快捷键都能在“设置 → 快捷键”里重新录制，工作区键统一附加 `!GpuiXterm` 上下文，避免在终端 TUI 中抢键。

---

## ✨ 功能特性

### 1\. 工作区与文件管理

- 三种空间各自记住标签页与阅读分栏：**默认空间**（用户目录下的 `.Folio` 文件夹）、**项目工作区**（添加文件夹后进入）、**独立文件**（`Ctrl+Shift+O` 打开的外部文件，始终从原路径读写）。
- 文件树支持新建 Markdown/文件/文件夹、重命名（文件/文件夹/项目）、删除、单击文件夹聚焦（影响新建落点与快捷操作）、`Ctrl+` 点击多选与空白处拖拽框选。
- **应用内拖拽**：把文件/文件夹拖到终端面板会把路径加引号后粘贴进终端；拖到废纸篓图标直接移入废纸篓。
- **快捷路径操作**（焦点在文件树）：`Ctrl+C` 复制选中项（或聚焦文件夹/当前文件）的完整路径，`Delete` 删除，`Ctrl+Z` 撤销删除。
- **废纸篓**：删除的文件先落盘到废纸篓（保留 7 天，启动时清理过期项），侧栏“废纸篓”面板可单项恢复、彻底删除或清空，误删可用 `Ctrl+Z` 整批撤销。
- 所有 Markdown 顶部有独立标题输入区：Enter 重命名同目录文件，Escape 取消；标题不写入正文，也不与正文一级标题联动。Windows 打包会将 Folio 加入 PDF / Markdown / LaTeX 的“打开方式”，设为默认应用需在系统“默认应用”设置中选择。

### 2\. LaTeX 编辑与编译

- 多标签页、语法高亮、行号开关、文本查找与替换、自动保存及外部修改冲突检测。
- 调用本机 TeX Live 的 `latexmk`，支持 XeLaTeX / pdfLaTeX / LuaLaTeX 三种引擎；顶栏选择主文件与引擎，`Ctrl+Enter` 触发或中止编译，完整构建日志单独成栏，失败时保留上一次的 PDF 预览。

### 3\. Markdown 混合编辑

- 所见即所得（块编辑器）与源码双模式，`Ctrl+/` 即时切换；斜杠菜单、选中工具条、文档大纲导航、可编辑表格、本地图片引用与独立编辑分屏。
- **GitHub Alerts**：引用块首行写 `[!NOTE]`、`[!TIP]`、`[!IMPORTANT]`、`[!WARNING]`、`[!CAUTION]`；可折叠变体 `[!NOTE]-` / `[!NOTE]+` 与 `<details><summary>标题</summary>正文</details>` 均可点击展开/收起，输入 `/` 可插入详情块。折叠状态是视图状态，PDF 导出始终显示完整正文。
- **表格操作**：单元格编辑、左/中/右对齐、增删行列、`Ctrl+Alt+↑/↓` 插入行、整表复制；`Ctrl+F` 在 WYSIWYG 文档内查找（自动填入选中文本，带匹配计数与上一个/下一个）。
- **代码块**：按围栏语言用 Syntect 高亮，顶部有可搜索语言下拉框与复制按钮（复制后短暂显示勾号），语言修改可撤销并写回 Markdown；块级代码随正文按固定比例缩放，用于预览与 PDF 导出。
- **数学语法**：原生支持 `$...$`、`$$...$$`、`\(...\)`、`\[...\]`，点击块级公式可直接就地编辑 LaTeX 源码。
- **emoji**：输入 `:` 加关键词调出 emoji 快捷插入（约 150 条，离线数据），斜杠菜单也有“表情符号”项；插入的是普通 Unicode 文本。
- **划词翻译**：WYSIWYG 选中文字后点工具条“译”按钮（PDF 内选中走同一浮层），先请求 Google、失败后回退百度；自动检测语言（中文→英，其余→中），带词典与复制反馈，凭据在“设置 → API 服务”配置。
- **链接跳转**：`Ctrl+左键`点击 Markdown 链接，外部链接用系统浏览器打开，工作区内的文件/目录/锚点在应用内打开；打开位置（主编辑区或右侧分屏）可在“设置 → 视图”里选择。

### 4\. PDF 预览与批注

- 连续滚动、页面跳转、多级缩放、目录树与侧边缩略图；每个分屏独立维护视图与滚动状态。
- 文本选择与复制、全文搜索、5 色荧光高亮与本地批注管理、划词翻译。

### 5\. HTML 交互预览

- 打开 `.html/.htm` 文件进入 WebView2 面板，用虚拟主机名把项目根映射为本地源，可加载同目录的 CSS / JS / 图片；工具栏提供后退、前进、刷新、在默认浏览器打开与关闭。仅预览时使用 WebView，数学、PDF 与导出均不经它。

### 6\. Markdown 导出 PDF

- 按当前编辑器正文宽度、字体设置和换行布局自动 A4 分页；矢量字形内嵌，正文与公式文字均可选可复制，导出不含光标与编辑态元素。

### 7\. 原生终端与编译日志

- 右上角日志图标显示构建输出（可复制全部）；“终端”按钮打开 Windows ConPTY 原生终端，默认启动加载 profile 的 PowerShell，可运行 `codex`、`agy` 等本机命令。
- 多会话标签、面板宽度拖动、最大化、隐藏（进程继续运行）；关闭会话或退出 Folio 时按进程树结束；终端使用 Folio 配色，切换明暗主题不重启 shell；复制/粘贴为 `Ctrl+Shift+C/V`，`Ctrl+C` 交给前台程序。
- 在“设置 → 终端”调整 shell、逐行启动参数与面板宽度；终端字体与字号在“设置 → 外观 → 字体”，即时应用到已有会话。

### 8\. 个性化设置

设置分为“外观（字体 / 主题 / 视图）”“快捷键”“终端”“API 服务”，改动即时应用并落盘：

- **主题**：浅色 / 深色 / 自定义，按颜色角色调整并命名保存、随时重置；深色参考暖黑、米白与陶土橙，切换主题保留字体与排版设置。
- **字体**：统一调整界面、编辑器、代码、数学与终端的字体和字号，均可搜索本机字体；行内代码默认为正文的 87.5%（可调 50%–150%），块级代码、数学字体独立设置，选择同时用于 PDF 导出。
- **视图**：源码行号、编辑器行宽限制、内部链接打开位置；`Ctrl+滚轮` 调整字号（限制 11–26 px，右下角显示当前值）。
- **快捷键**：录制/改键，详见下表。
- **API 服务**：百度翻译 APP ID / API Key（划词翻译在 Google 不可用时回退百度）。

---

## ⌨️ 常用快捷键

以下为**默认**值，均可在“设置 → 快捷键”中修改（`src/model.rs` 的 `SHORTCUT_DEFS`）。工作区快捷键在终端 TUI 内不拦截。

| 快捷键 | 功能说明 |
| :--- | :--- |
| `Ctrl + S` | 保存当前文件 |
| `Ctrl + Alt + S` | 保存全部文件 |
| `Ctrl + O` | 打开本地项目文件夹 |
| `Ctrl + Shift + O` | 打开本地 PDF / Markdown / LaTeX 文件 |
| `Ctrl + Enter` | 触发 / 停止编译 |
| `Ctrl + \` | 显示 / 隐藏左侧栏 |
| `Ctrl + P` | 查找项目内文件 |
| `Ctrl + W` | 关闭当前标签页 |
| `Ctrl + /` | 切换 Markdown 源码 / 所见即所得 |
| `Ctrl + F` | WYSIWYG 文档内查找 |
| `Ctrl + B` / `I` / `U` | 加粗 / 斜体 / 下划线 |
| `Ctrl + Shift + S` | 删除线 |
| `Ctrl + E` | 行内代码 |
| `Ctrl + Shift + H` | 荧光笔高亮 |
| `Ctrl + K` | 插入 / 编辑链接 |
| `Ctrl + 滚轮` | 调整编辑器字号 / Markdown 缩放 / PDF 缩放 |
| `Esc` | 关闭弹窗、设置面板或退出查找 |
| 文件树内 `Ctrl + C` | 复制选中项完整路径 |
| 文件树内 `Delete` / `Ctrl + Z` | 删除到废纸篓 / 撤销删除 |
| 表格内 `Ctrl + Alt + ↑ / ↓` | 在上方 / 下方插入行 |

---

## 🛠️ 构建与运行

### 桌面版（Windows）

前置环境：

1. **操作系统**：Windows 10 / 11（x86_64）。
2. **Rust 工具链**：`edition = 2024` 需 Rust 1.85+；构建脚本把链接器设为 `gcc`，因此必须安装 **GNU 工具链（`stable-x86_64-pc-windows-gnu`）与 MinGW-w64**（`gcc` 在 `PATH` 中），MSVC 工具链不适用。
3. **WebView2 Runtime**：HTML 预览面板依赖系统 WebView2（Win11 通常已内置），打包脚本会一并带上 `WebView2Loader.dll`。
4. **TeX Live**（可选）：整篇 `.tex` 编译需要本机 TeX Live；纯 Markdown 编辑、预览与 PDF 导出无需安装。

```powershell
# 编译并以发布模式运行（产物 target\release\folio-gpui-prototype.exe）
cargo run --release

# 一键：编译 + 打包 + 建桌面快捷方式 + 注册文件关联 + 启动
.\编译并更新桌面.cmd

# 仅打包到 dist\releases\（可加 -Launch 启动、-SkipBuild 跳过编译）
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\package.ps1

# 启动稳定目录中的 Folio（参数透传，如 --open <path>）
.\启动 Folio.cmd
```

`scripts/package.ps1` 会把可执行文件、`WebView2Loader.dll`、图标、字体与许可证打包到带时间戳的 `dist\releases\Folio-<时间>\`，同时更新稳定目录 `dist\Folio\`，并在桌面创建“Folio GPUI”快捷方式。文件关联只注册当前用户，可用 `scripts/register-file-associations.ps1 -Unregister` 撤销。仓库内的 `runtime/` 与 PDFium 解包脚本是旧版遗留，当前 PDF 渲染不依赖 PDFium。

### Android 版

独立于桌面 Rust 工程，详见 [android/README.md](android/README.md) 与 [android/README-native.md](android/README-native.md)。构建需 JDK 17、Gradle 8.11.1、Android SDK 35、Node.js：

```powershell
.\scripts\build-android.ps1
```

产物为 `dist\android\Folio-Android-0.1.0.apk`（最低 Android 8.0 / API 26）。Android 版只做 Markdown 阅读与编辑（ProseMirror + KaTeX），不含桌面版的 LaTeX 编译、PDF 批注、原生终端和多分屏。

### 测试

```powershell
cargo test                       # Rust：tests/fold_callouts.rs、tests/appearance.rs
cd android/web; npm ci; npm test; npm run test:ui; npm run build   # Android 前端
```

Rust 集成测试覆盖折叠标注的 Markdown 往返与输入规则、主题/自定义配色持久化及块级代码随正文缩放；Android 前端测试覆盖公式分隔符、转义与浏览器端交互。打包脚本本身不运行测试。

---

## 📁 目录结构

```text
├── src/                  # 桌面 Rust 主程序（workspace/markdown/pdf/html/translate/trash/terminal 等 20+ 模块）
├── vendor/               # vendored 与打补丁的 crate：latex-rust、gpui-pdf、gpui-xterm、
│                         #   gpui-kit-markdown-editor、gpui-pre(-windows)；补丁见 FOLIO-PATCHES.md
├── resources/            # 应用图标、编译期内嵌的数学字体、katex 资源
├── scripts/              # 打包 / Android 构建 / 文件关联 / PDFium 辅助脚本
├── runtime/              # PDFium 解包产物（旧版遗留，当前渲染不使用）
├── dist/                 # 发布产物（releases/、稳定目录 Folio/、android/ APK）
├── android/              # 独立 Android 工程（原生外壳 + web 前端）
├── docs/                 # 技术调研与规范（公式排版评测、主题、终端方案）
├── tests/                # Rust 集成测试
├── fixtures/             # 冒烟样例（.tex / .md / .pdf）
├── 启动 Folio.cmd          # 启动 dist\Folio\Folio.exe
├── 编译并更新桌面.cmd      # 一键编译 + 打包 + 启动
└── MIGRATION.md          # Electron → GPUI 迁移清单
```

---

## 📌 说明

- **自用分享**：按个人写作与排版习惯搭建，主要满足日常 LaTeX 编译与 Markdown 记笔记需求。
- **数据存储**：配置保存在 `%APPDATA%\folio-gpui`（可用 `FOLIO_USER_DATA` 覆盖），含 `preferences.json`、`projects.json`、`workspace-sessions.json`、废纸篓与 WebView2 缓存；PDF 批注在目标目录下的 `.folio/annotations.json`；文档源文件与编译 PDF 均留在项目本地目录，不改动用户文件结构。
- **隐私**：划词翻译会把选中文本发送给 Google（或其百度回退）服务；启用前请确认这符合你对相关内容的处理要求。

