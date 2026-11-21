# Folio (GPUI)

基于 [GPUI](https://www.gpui.rs/) 与 Rust 编写的 Windows 原生 LaTeX / Markdown 个人工作台。

写文档和论文时，希望有一款轻量、启动快、内存占用低的本地编辑工具，于是用 GPUI 做了这套原生工作台。界面、编辑器、公式排版、PDF 预览与导出均在进程内完成；AI 助手同样是原生 GPUI 面板，只把模型与工具宿主放到独立的 Node 进程；打开 `.html` 文件时才按需调用系统 WebView2。

---

## 💡 设计特点

- **原生渲染为主**：界面、编辑器、数学公式与 PDF 的排版/预览/导出全部基于 GPUI 原生渲染，不打包 Electron/Chromium。WebView 只用于两处按需场景：打开 `.html/.htm` 与离线 draw.io 绘图，均复用**系统 WebView2**（Edge Chromium 内核），惰性创建、关闭即释放；draw.io 资源随应用打包并经虚拟宿主映射，不访问在线服务。
- **进程内数学公式排版**：Markdown 内嵌的 LaTeX 公式基于 `latex-rust` 盒模型在进程内解析与布局，直接由 GPUI 绘制字形与矢量横线。内置 STIX Two、Latin Modern、TeX Gyre Pagella 三套数学字体，中文缺字回退系统字体。
- **矢量 PDF 导出**：Markdown 导出 PDF 复用文档绘制操作并通过 `krilla` 输出，正文与公式均保留为可选择、可复制的矢量文字，不调用无头浏览器。
- **纯 Rust PDF 预览**：通过 vendored `gpui-pdf`（渲染后端 `hayro`）实现页面虚拟化、按需光栅化、目录导航、缩略图与高亮批注；文本层支持搜索、选择与复制。
- **克制的资源管理**：PDF 仅缓存视口附近页面，缩放或改窗口时丢弃未完成的旧渲染任务，隐藏标签/分屏时释放资源；文件监听（`notify`）只比对当前打开文件，目录结构变动才重扫文件树；编辑器草稿随击键更新，不做定时全量轮询。
- **可配置快捷键**：所有工作区与 Markdown 快捷键都能在“设置 → 快捷键”里重新录制，工作区键统一附加 `!GpuiXterm` 上下文，避免在终端 TUI 中抢键。
- **原生 AI 面板**：会话界面由 GPUI 绘制，模型与工具通过独立的 Node SDK 宿主运行；按项目多会话并行，切换标签或项目不打断任务，宿主随 Folio 退出由 Windows 作业对象回收。

---

## ✨ 功能特性

### 1\. 工作区与文件管理

- 三种空间各自记住标签页与阅读分栏：**默认空间**（用户目录下的 `.Folio` 文件夹）、**项目工作区**（添加文件夹后进入）、**独立文件**（`Ctrl+Shift+O` 打开的外部文件，始终从原路径读写）。
- 文件树默认折叠并记忆展开状态，永久忽略 `.pi/`；左侧栏「新建」拆分按钮可新建 `.md` / `.tex` / `.drawio` / `.bib` / `.txt` 或其他类型，也可新建文件夹；支持重命名（文件/文件夹/项目）、删除、单击文件夹聚焦（影响新建落点与快捷操作）、按住 `Ctrl` 点击多选与空白处拖拽框选。
- **应用内拖拽**：把文件/文件夹拖到终端面板会把路径加引号后粘贴进终端；拖到废纸篓图标直接移入废纸篓；同一项目内可拖拽文件夹移动位置。
- **快捷路径操作**（焦点在文件树）：`Ctrl+C` 复制选中项（或聚焦文件夹/当前文件）的完整路径，`Delete` 删除，`Ctrl+Z` 撤销删除。
- **废纸篓**：删除的文件先落盘到废纸篓（保留 7 天，启动时清理过期项），侧栏“废纸篓”面板可单项恢复、彻底删除或清空，误删可用 `Ctrl+Z` 整批撤销。
- 所有 Markdown 顶部有独立标题输入区：Enter 重命名同目录文件，Escape 取消；标题不写入正文，也不与正文一级标题联动。Windows 打包会将 Folio 加入 PDF / Markdown / LaTeX 的“打开方式”，设为默认应用需在系统“默认应用”设置中选择。
- **文件标签（Tag）**：标签挂在具体文件上（六色：灰 / 蓝 / 绿 / 橙 / 红 / 紫），文件夹只作为批量编辑的范围。入口有侧栏「文件」标题栏的标签按钮、文件树右键菜单（单文件 / 文件夹递归 / 多选）以及文件树聚焦时的 `Alt+T`；点击标签名或文件行上的标签 chip 可按标签筛选文件树。
- **标签维护**：面板中可新建、重命名、改色或删除标签（删除会从所有关联文件移除），文件移动 / 改名后可在「待重新关联」区重新绑定。标签存 SQLite：项目为 `<项目>/.folio/metadata.sqlite3`，默认空间为 `~/.Folio/.folio/metadata.sqlite3`，独立文件为应用数据目录下的 `loose-metadata.sqlite3`。
- **Zen 专注模式**（`F11`）：一键隐藏左侧栏、右侧功能中心、终端、日志与构建预览并进入全屏，只留编辑器与文档大纲；再次按 `F11` 或点击「退出 Zen」恢复进入前的面板与全屏状态。可从右侧功能中心搜索「Zen 专注模式」进入，「设置 → 快捷键」可改键。

### 2\. LaTeX 编辑与编译

- 多标签页、语法高亮、行号开关、文本查找与替换、自动保存及外部修改冲突检测。
- 调用本机 TeX Live 的 `latexmk`，支持 XeLaTeX / pdfLaTeX / LuaLaTeX 三种引擎；顶栏选择主文件与引擎，`Ctrl+Enter` 触发或中止编译，完整构建日志单独成栏，失败时保留上一次的 PDF 预览。

### 3\. Markdown 混合编辑

- 所见即所得（块编辑器）与源码双模式，`Ctrl+/` 即时切换；斜杠菜单、选中工具条、文档大纲导航、可编辑表格、本地图片引用与独立编辑分屏。
- **文档大纲**：视觉模式下宽窗口（≥1050px）自动展开左侧 240px 大纲栏，窄窗口（\<900px）收起为浮层；工具栏大纲按钮可手动切换。大纲按层级展示、可折叠父节点，点击条目滚动定位并短暂高亮标题，当前章节及其折叠祖先会高亮标识；独立编辑分屏中同样可用。
- **GitHub Alerts**：引用块首行写 `[!NOTE]`、`[!TIP]`、`[!IMPORTANT]`、`[!WARNING]`、`[!CAUTION]`（另有 `info`、`abstract`、`success`、`question`、`todo`、`danger`、`error`、`example`、`quote` 等别名）；可折叠变体 `[!NOTE]-` / `[!NOTE]+`（可带自定义标题）与 `<details><summary>标题</summary>正文</details>` 均可点击展开/收起，输入 `/` 可插入详情块。折叠状态是视图状态，PDF 导出始终显示完整正文。
- **表格操作**：单元格编辑、左/中/右对齐、增删行列、`Ctrl+Alt+↑/↓` 插入行、整表复制。多行表格的控制栏提供折叠按钮，折叠后只显示仍可编辑的表头行；与 `[!NOTE]-` 的文档级折叠不同，属于视图状态，不写回 Markdown。
- **查找与替换**：所见即所得模式（源码模式不支持）下 `Ctrl+F` 查找、`Ctrl+H` 替换；打开时自动填入当前选中文本，带 `1/3` 形式的匹配计数、上一个 / 下一个（`Shift+Enter` / `Enter`）、区分大小写、全字匹配、替换当前与全部替换。`Ctrl+H` 为 WYSIWYG 专用快捷键，不在“设置 → 快捷键”中重录制。
- **代码块**：按围栏语言用 Syntect 高亮，顶部有可搜索语言下拉框与复制按钮（复制后短暂显示勾号），语言修改可撤销并写回 Markdown；块级代码随正文按固定比例缩放，用于预览与 PDF 导出。代码块头部另有折叠按钮，默认展开，折叠后仅保留语言与复制工具栏；状态不写入 Markdown，PDF 导出始终完整。
- **Mermaid 图表（桌面）**：`mermaid` 围栏由 Rust 引擎 `merman` 在后台解析与布局为 SVG，`resvg` 光栅化为可缩放预览，同一 SVG 直接写入 PDF；默认图表预览，可就地切换源码，保存仍为 Markdown 围栏。支持缩放与平移，不引入 JS、WebView 或在线服务；兼容范围受 Rust 引擎限制，详见 [Markdown 写作提示](FOLIO_MARKDOWN.md#mermaid-图表桌面)。
- **数学语法**：原生支持 `$...$`、`$$...$$`、`\(...\)`、`\[...\]`，点击块级公式可在弹窗中就地编辑 LaTeX 正文（无需分隔符）。支持分式、根式、上下标、重音、字体与颜色，以及 `matrix` / `pmatrix` / `bmatrix` / `cases` / `array`、`align` / `gather` / `multline` / `equation`（含 `\tag`、`\label`、`\ref`）及星号无编号版本；完整语法与不支持项见 [LaTeX 支持边界](LATEX_SUPPORT_BOUNDARIES.md) 与 [扩展指南](LATEX_EXTENSION_GUIDE.md)。
- **HTML 表格**：Markdown 中粘贴或手写原生 `<table>`（`thead` / `tbody` / `tfoot`、`td` / `th`、`rowspan` / `colspan`、`text-align`）在视觉模式下原生渲染为可编辑单元格；单元格内容按与正文相同的 Markdown 解析（`**加粗**`、`` `代码` ``、链接、`$...$` 公式等）并随正文导出到矢量 PDF；双击单元格编辑时原始 HTML 标签被转义，不可渲染的 `script` / `style` 文本被剔除但源文件完整保留以便往返。
- **emoji**：输入 `:` 加关键词调出 emoji 快捷插入（约 150 条，离线数据），斜杠菜单也有“表情符号”项；插入的是普通 Unicode 文本。
- **划词翻译**：WYSIWYG 选中文字后点工具条“译”按钮（PDF 内选中走同一浮层），先请求 Google、失败后回退百度；自动检测语言（中文→英，其余→中），带词典与复制反馈，凭据在“设置 → API 服务”配置。
- **链接跳转**：`Ctrl+左键`点击 Markdown 链接，外部链接用系统浏览器打开，工作区内的文件/目录/锚点在应用内打开；打开位置（主编辑区或右侧分屏）可在“设置 → 视图”里选择。
- **剪贴板图像与 SVG**：视觉 / 源码模式 `Ctrl+V` 检测到图像或 SVG 文本（如 draw.io「Copy as SVG」）时，自动落盘到 `images/<文档名>/` 并插入 `![图片](<相对路径>)`；png / jpg / webp / gif / svg 原样保存，bmp / tiff / ico / pnm 转为 PNG。打开图片文件进入查看器：`Ctrl+滚轮` 以光标为锚点缩放，`Ctrl+=` / `Ctrl+-` 缩放，`Ctrl+0` 适应窗口，`Ctrl+1` 实际大小，滚轮上下平移、拖拽移动、双击切换 100%。

### 4\. PDF 预览与批注

- 连续滚动、页面跳转、多级缩放、目录树与侧边缩略图；每个分屏独立维护视图与滚动状态。
- 文本选择与复制、全文搜索、5 色荧光高亮与本地批注管理、划词翻译。
- **批注管理**：选中文字或 `Alt` 拖拽框选即可高亮并写评论；批注集中在右侧栏的卡片中就地查看、编辑、改色与删除。卡片采用与高亮色一致的淡色底与描边（选中 / 编辑时加深），评论可多行自动换行、卡片高度自适应，点击卡片会在原文中定位到对应高亮。AI 助手也能为 PDF 添加同类批注，见第 9 节。

### 5\. HTML 交互预览

- 打开 `.html/.htm` 文件进入 WebView2 面板，用虚拟主机名把项目根映射为本地源，可加载同目录的 CSS / JS / 图片；工具栏提供后退、前进、刷新、在默认浏览器打开与关闭。仅预览时使用 WebView，数学、PDF 与导出均不经它。

### 6\. 离线 Draw.io 绘图

- 内置离线 draw.io 静态资源（`resources/drawio/`，`scripts/install-drawio.ps1` 可拉取 v32.7.4），通过 WebView2 虚拟宿主 `folio-drawio.localhost` 在进程内打开；画布固定简体中文、预置 10px 浅色网格，不访问在线服务（外链交给系统浏览器）。
- 左侧栏「新建」下拉的「新建 .drawio」可创建原生 `.drawio`（也识别 `.drawio.svg`）文件并保存在项目内；编辑器支持自动保存与外部改动检测，工具栏可导出 PDF / SVG / PNG 伴生文件。
- AI 可用 `folio_drawio_read` / `folio_drawio_create` / `folio_drawio_update` 读写整份 XML，或用 `folio_drawio_get_selection` / `folio_drawio_edit_cells` 按选区与元素 ID 在一次事务内增删 / 更新节点与连线并自动保存；由内置 `folio-drawio` 技能指导，调用无需审批。详见 [docs/ai-runtime.md](docs/ai-runtime.md#drawio-tools)。

### 7\. Markdown 导出 PDF

- 按当前编辑器正文宽度、字体设置和换行布局自动 A4 分页；矢量字形内嵌，正文与公式文字均可选可复制，Mermaid 等待全文布局完成后以原生路径与字形输出，导出不含光标与编辑态元素。

### 8\. 原生终端与编译日志

- 右上角日志图标显示构建输出（可复制全部）；“终端”按钮打开 Windows ConPTY 原生终端，默认启动加载 profile 的 PowerShell，可运行 `codex`、`agy` 等本机命令。
- 多会话标签、面板宽度拖动、最大化、隐藏（进程继续运行）；关闭会话或退出 Folio 时按进程树结束；终端使用 Folio 配色，切换明暗主题不重启 shell；复制/粘贴为 `Ctrl+Shift+C/V`，`Ctrl+C` 交给前台程序。
- 在“设置 → 终端”调整 shell、逐行启动参数与面板宽度；终端字体与字号在“设置 → 外观 → 字体”，即时应用到已有会话。
- **右侧功能中心**：右侧面板可搜索功能、命令与文件（AI 助手、终端、PDF 预览、编译日志、打开文件、偏好设置），集中显示编译 / AI / 终端的“运行中任务”；支持拖拽调宽、最大化 / 还原，并可在 AI 与终端等视图间切换。

### 9\. AI 助手（原生面板）

`Ctrl+Shift+A` 打开 AI 面板。会话基于 Pi SDK 的独立 Node 宿主，按项目隔离、多标签并行；切换项目或关闭标签不会中止正在运行的任务，关闭 Folio 时宿主与子进程树通过 Windows 作业对象一起回收。网关与模型在 `设置 → API 服务 → AI 服务（Magpie 网关）` 配置：Base URL、标题模型 ID（默认 `workbuddy-ai/hy4flash-free`）、Exa API Key 与百度千帆 API Key（掩码存储，不进入工具参数）。

- **会话管理**：每个项目的标签与激活状态单独持久化（应用数据目录下的 `ai-tabs.json`），Pi JSONL 仍是对话与名称的唯一来源，Folio 另存置顶与标题来源（`ai-session-metadata.json`）；恢复的标签只在激活时加载，中断的任务不会自动重投。第一条纯文本消息触发一次自动标题，手动命名优先；会话支持置顶、历史搜索/重命名，左侧消息导航条可跳转到用户消息或某次回答。删除运行中的会话需要二次确认；关闭标签只是释放前台占用、保留后台运行。
- **输入与输出**：可附加当前文件、引用 WYSIWYG/PDF 选区、粘贴剪贴板图片、挂载技能，并选择模型、思考级别与收藏模型；`/` 呼出命令（开关当前上下文、压缩上下文、挂载技能）。在 AI 回答中拖选文字会浮出「添加到对话」按钮，可把该段回答作为引用继续追问，引用以 `> 引用自 …` 块随消息发送。工具调用参数、文件改动 diff、思考过程与耗时分层展示并可展开查看；diff 显示文件真实行号，相隔较远的未改动上下文折叠为分隔线（保留改动前后各 2 行）。消息较长时右下角出现「回到底部」圆形按钮，点击滚到最新并恢复自动跟随；手动上滚会暂停跟随。回答可复制或导出为笔记（同名 Markdown 写入会话所属项目根目录，文件名用标题+时间戳，并自动打开，不覆盖已有文件）。
- **联网与抓取**：`exa_search`（web/academic、时间与域名过滤）和 `baidu_qianfan_search`（中文来源）是显式工具选择；Exa 通过 PowerShell 7/.NET 读取 Windows 系统代理并用 Undici dispatcher 发起请求，未配置代理时直接报错、不回退直连。`web_fetch` 用 Scrapling 抽取 Markdown，`mode: dynamic` 显式启动临时 Chromium；PDF 支持按 1 开始的页码选取，扫描/混合 PDF 返回待 OCR 页码，交给独立的 `paddleocr` MCP 服务（PaddleOCR-VL-1.6）处理，不是内嵌模型或隐藏下载。
- **工作区工具**：AI 可查看 / 按标签查找当前项目或默认空间内的文件，为文件增删标签、给新标签指定颜色，以及全局重命名标签 / 改色（`folio_tags_list` / `folio_files_query` / `folio_file_tags_update` / `folio_tags_manage`）；`run_python` 将多行 Python 源码直接经 stdin 交给 `python -X utf8 -u` 执行（不经过 shell，避免转义问题，返回合并输出与退出码）。标签改动不经审批，直接在会话时间线生成带「撤销」按钮的回执；仅项目与默认空间可用，独立文件空间不开放。
- **文档批注**：可直接让 AI 为当前项目的 Markdown 或有文字层的 PDF 写批注（`folio_annotations_list` / `folio_annotation_create`）。AI 选取少量有价值位置添加高亮与评论，PDF 页码从 1 起、重复原文可用 occurrence 定位，已有不同评论不会被覆盖；首版不支持扫描页 / OCR、公式与图片区域、跨块锚点及原生 PDF / Word 批注导出。批注写入文档旁的 `.folio/annotations.json`（与手动批注共用），Markdown 高亮写回正文；正文保存成功而 sidecar 失败时明确提示。工作流由内置的 `folio-annotations` 技能指导。
- **上下文与权限**：会话顶部显示上下文窗口用量；思考级别可调（关闭 / 最低 / 低 / 中 / 高 / 极高 / 最高，模型不支持时提示“当前模型不支持调节”）。命令与脚本执行前弹出审批卡片（“允许” / “运行命令”，支持“此会话始终允许”与“拒绝”），顶栏盾牌图标切换“需确认” ↔ “完全访问”；扩展的 confirm / select / input / editor 提示也以卡片呈现。工具时间线的详略由“设置 → 视图 → 工具执行视图”（极简 / 标准 / 详细）控制。
- **文档选页预览**：可读取 Word / PowerPoint / PDF 的指定页，页码从 1 开始；先用 `scripts/install-document-renderers.ps1` 把渲染脚本装入本机 Magpie 的 `pdf`/`docx`/`pptx` 技能，Office 格式依赖本机 Microsoft Office，结果以 PNG 供模型查看。
- **隐私**：输入、附件与选区会发送到配置的网关和所选联网服务；每次运行还会追加一条隐藏的本地日期/时间与时区上下文到 Pi JSONL。工具结果区分 `ok` / `empty` / `error` / `cancelled`，凭证不出现在参数或结果里。详见 [docs/ai-runtime.md](docs/ai-runtime.md)。
- **批量 TeX 编译**：`folio_tex_compile` 调用 Folio 内置 `latexmk` 管线，把一个或多个 `.tex` 编译为同目录同名 PDF；`paths`（1–50 个具体文件）与 `under_directory`（递归，`"."` 为项目根）二选一，`engine` 可选（`xelatex` / `pdflatex` / `lualatex`，省略沿用项目设置）。后台最多 4 个并行、与界面编译槽隔离，可取消、无需审批；AI 应优先用它而非 shell 直接调 `latexmk`。
- **工具桥接与扩展**：宿主与 Folio 通过统一的 `workspace_tool_request` / `workspace_tool_cancel` / `workspace_tool_response` 协议通信；新增工具的目录结构与打包规则见 [resources/pi-host/README.md](resources/pi-host/README.md)。

### 10\. 个性化设置

设置分为“外观（字体 / 主题 / 视图）”“快捷键”“终端”“API 服务”，改动即时应用并落盘：

- **主题**：浅色 / 深色 / 自定义，12 个颜色角色可逐项调整并命名保存、随时重置；深色参考暖黑、米白与陶土橙，切换主题保留字体与排版设置。“画布与弹窗”颜色同时作为 AI 面板底色，切换或自定义配色时联动；主题仅含颜色，字体与排版独立。
- **字体**：按区域独立设置字体与字号——界面（工具栏 / 侧栏 / 设置，11–20 px）、源码编辑器、Markdown 预览、AI 面板、终端（均 11–26 px），均为可搜索的本机字体选择器，默认 14 px。“Markdown 与 AI 共享排版”中可设置行内 / 块级代码字体与字号比例（0.5×–1.5×，默认 87.5% / 92.9%）、数学字体（STIX Two / Latin Modern / TeX Gyre Pagella）与行内 / 块级公式比例（0.5×–3×，默认 1.0× / 1.7×）。“字体回退”可指定全局缺字回退字体，并调整“中西文字体比例”（80%–140%，默认 100%，按比例缩放中文及全角标点以平衡中西文视觉大小）。“恢复默认字体”一键还原全部字体与比例。
- **视图**：`界面语言`（简体中文 / English）、`背景图` 与 `图片不透明度`（0–100%，默认 35%，设为 0 时隐藏）、源码行号、编辑器行宽限制、内部链接打开位置、`工具执行视图`（极简 / 标准 / 详细，控制 AI 时间线的工具细节层级）与终端面板宽度（280–1600 px）；`Ctrl+滚轮` 调整字号（11–26 px，右下角显示当前值）。
- **快捷键**：录制 / 改键，详见下表。
- **终端**：shell、逐行启动参数与面板宽度；终端字体与字号在“外观 → 字体”设置，即时应用到已有会话。
- **API 服务**：百度翻译 APP ID / API Key（划词翻译在 Google 不可用时回退百度）；AI 网关 Base URL、标题模型 ID、Exa / 千帆 API Key，详见第 9 节 AI 助手。

---

## ⌨️ 常用快捷键

以下为**默认**值，均可在“设置 → 快捷键”中修改（`src/model.rs` 的 `SHORTCUT_DEFS`）。工作区快捷键在终端 TUI 内不拦截，但 AI 面板与终端开关在终端聚焦时仍可用（见下表）。

| 快捷键 | 功能说明 |
| :--- | :--- |
| `Ctrl + S` | 保存当前文件 |
| `Ctrl + Alt + S` | 保存全部文件 |
| `Ctrl + O` | 打开本地项目文件夹 |
| `Ctrl + Shift + O` | 打开本地 PDF / Markdown / LaTeX 文件 |
| `Ctrl + Enter` | 触发 / 停止编译 |
| `Ctrl + \` | 显示 / 隐藏左侧栏 |
| `Ctrl + P` | 查找项目内文件 |
| `Ctrl + Shift + A` | 打开 / 收起 AI 助手 |
| `F11` | 进入 / 退出 Zen 专注模式 |
| `` Ctrl + ` `` | 打开 / 收起终端 |
| `Ctrl + W` | 关闭当前标签页 |
| `Ctrl + /` | 切换 Markdown 源码 / 所见即所得 |
| `Ctrl + F` | WYSIWYG 文档内查找 |
| `Ctrl + H` | WYSIWYG 文档内替换（源码模式不支持） |
| `Ctrl + B` / `I` / `U` | 加粗 / 斜体 / 下划线 |
| `Ctrl + Shift + S` | 删除线 |
| `Ctrl + E` | 行内代码 |
| `Ctrl + Shift + H` | 荧光笔高亮 |
| `Ctrl + K` | 插入 / 编辑链接 |
| `Ctrl + 滚轮` | 调整编辑器字号 / Markdown 缩放 / PDF 缩放 |
| 图片查看器内 `Ctrl+滚轮` / 拖拽 / `Ctrl+0` / `Ctrl+1` | 缩放 / 平移 / 适应窗口 / 实际大小（100%） |
| `Esc` | 关闭弹窗、设置面板或退出查找 |
| 文件树内 `Ctrl + C` | 复制选中项完整路径 |
| 文件树内 `Delete` / `Ctrl + Z` | 删除到废纸篓 / 撤销删除 |
| 文件树内 `Alt + T` | 批量编辑文件标签 |
| 表格内 `Ctrl + Alt + ↑ / ↓` | 在上方 / 下方插入行 |

---

## 🛠️ 构建与运行

### 桌面版（Windows）

前置环境：

1. **操作系统**：Windows 10 / 11（x86_64）。
2. **Rust 工具链**：`edition = 2024` 需 Rust 1.85+；构建脚本把链接器设为 `gcc`，因此必须安装 **GNU 工具链（`stable-x86_64-pc-windows-gnu`）与 MinGW-w64**（`gcc` 在 `PATH` 中），MSVC 工具链不适用。
3. **WebView2 Runtime**：HTML 预览与离线 draw.io 绘图面板依赖系统 WebView2（Win11 通常已内置），打包脚本会一并带上 `WebView2Loader.dll`。
4. **TeX Live**（可选）：整篇 `.tex` 编译需要本机 TeX Live；纯 Markdown 编辑、预览与 PDF 导出无需安装。
5. **AI 助手**（可选功能、打包必需）：系统 `node >= 22.19.0` 在 `PATH` 中，`uv` 可用（`uv venv --python 3.12` 准备隔离的网页抓取环境）；运行时须保留系统 Node 与创建虚拟环境用的 Python 基础解释器。先运行 `scripts/setup-ai.ps1`，否则 AI 面板会提示宿主依赖未安装，`package.ps1` 也会拒绝打包。
6. **文档选页渲染**（可选）：Word / PPT 的选页预览需要本机 Microsoft Office；运行 `scripts/install-document-renderers.ps1` 把脚本装入 Magpie 技能库（默认 `%USERPROFILE%\.config\magpie\library\skills`）。
7. **draw.io 资源**（已随仓库内置）：`resources/drawio/` 为离线静态资源；需要更新版本时运行 `scripts/install-drawio.ps1` 重新拉取 v32.7.4。

```powershell
# 编译并以发布模式运行（产物 target\release\folio-gpui-prototype.exe）
cargo run --release

# 一键：编译 + 打包 + 建桌面快捷方式 + 注册文件关联 + 启动
.\编译并更新桌面.cmd

# 仅打包到 dist\releases\（可加 -Launch 启动、-SkipBuild 跳过编译）
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\package.ps1

# 安装 AI 宿主依赖（AI 面板或打包前必须）；-InstallBrowser 安装动态抓取用的 Chromium
.\scripts\setup-ai.ps1
# 可选：安装 PDF/Word/PPT 选页渲染脚本到本机 Magpie 技能库
.\scripts\install-document-renderers.ps1
# 更新 draw.io 离线资源（仓库已内置，通常无需执行）
.\scripts\install-drawio.ps1

# 启动稳定目录中的 Folio（参数透传，如 --open <path>）
.\启动 Folio.cmd
```

`scripts/package.ps1` 会把可执行文件、`WebView2Loader.dll`、图标、字体与许可证打包到带时间戳的 `dist\releases\Folio-<时间>\`，同时更新稳定目录 `dist\Folio\`，并在桌面创建“Folio GPUI”快捷方式。它还会把 `pi-host/`（含 `node_modules`、`.venv` 与内置技能）和 `drawio/` 离线资源复制到发布与稳定目录，因此打包前必须先跑 `setup-ai.ps1` 并确保 `resources/drawio/` 存在（缺失则拒绝打包）；`-KeepRunning` 可在 Folio 仍运行时跳过关闭检查。文件关联只注册当前用户，可用 `scripts/register-file-associations.ps1 -Unregister` 撤销。

### 测试

```powershell
cargo test                       # Rust：tests/fold_callouts.rs、tests/appearance.rs、tests/annotations.rs
npm test --prefix resources/pi-host   # AI 宿主 JSONL 协议、标签 / 批注 / 编译 / draw.io 工具（受控 OpenAI 兼容服务）
```

Rust 集成测试覆盖折叠标注的 Markdown 往返与输入规则、主题/自定义配色持久化、块级代码随正文缩放，以及批注的 Markdown / PDF 定位、歧义拒绝与越界路径防护；AI 宿主测试覆盖多会话并行、实时快照、项目隔离、标题与删除保护、联网工具选项与取消，以及标签 / 批注工具的请求—应答往返与会话隔离，不使用固定等待。打包脚本本身不运行测试。

---

## 📁 目录结构

```text
├── src/                       # 桌面 Rust 主程序入口、共享模型与设置（main.rs / model.rs / theme.rs / storage.rs 等）
├── src/workspace/             # 工作区：文件树、项目、构建生命周期、文档状态、外观同步、标签与批注
├── src/markdown.rs            # Markdown 编辑器入口
├── src/markdown/              # 所见即所得视图、大纲、emoji
├── src/ai/                    # 原生 AI 会话 UI、宿主 RPC、渲染、diff、引用与审批
├── src/terminal/              # ConPTY 原生终端会话与主题
├── src/web_view/              # WebView2 基础设施与 HTML / draw.io 面板（mod.rs / html.rs / drawio.rs）
├── src/image/                 # 剪贴板图片/SVG 落盘与可缩放图片查看器（clipboard.rs / viewer.rs）
├── src/tags.rs / src/tags/    # 文件标签 SQLite 存储与 AI 工具
├── src/i18n/                  # 中英双语文案
├── vendor/                    # vendored 与打补丁的 crate：latex-rust、gpui-pdf、gpui-xterm、
│                              #   gpui-kit-markdown-editor、gpui-pre(-windows)；补丁见 FOLIO-PATCHES.md
├── resources/                 # 应用图标与编译期内嵌的数学字体
├── resources/pi-host/         # Pi SDK Node 宿主、网页抓取 / Python / 标签 / 批注 / 编译 / draw.io 工具、内置技能与扩展指南（README.md）
├── resources/drawio/          # draw.io v32.7.4 离线静态资源（WebView2 虚拟宿主映射）
├── resources/pi-skills/       # PDF/Word/PPT 选页渲染脚本与 Windows 说明
├── scripts/                   # 打包、AI 宿主与文档渲染安装、draw.io 资源拉取、文件关联
├── dist/                      # 发布产物（releases/、稳定目录 Folio/）
├── docs/                      # 技术调研与规范（AI 运行时、公式排版评测、主题、终端方案）
├── tests/                     # Rust 集成测试（折叠标注、外观持久化、批注定位）
├── fixtures/                  # 冒烟样例（.tex / .md / .pdf）
├── FOLIO_MARKDOWN.md          # Markdown 写作提示（Alerts、Mermaid、表格等）
├── LATEX_SUPPORT_BOUNDARIES.md / LATEX_EXTENSION_GUIDE.md  # 数学公式支持范围与扩展指南
├── 启动 Folio.cmd              # 启动 dist\Folio\Folio.exe
├── 编译并更新桌面.cmd          # 一键编译 + 打包 + 启动
└── MIGRATION.md                # Electron → GPUI 迁移清单
```

---

## 📌 说明

- **自用分享**：按个人写作与排版习惯搭建，主要满足日常 LaTeX 编译与 Markdown 记笔记需求。
- **数据存储**：配置保存在 `%APPDATA%\folio-gpui`（可用 `FOLIO_USER_DATA` 覆盖），含 `preferences.json`、`projects.json`、`workspace-sessions.json`、`ai-tabs.json`、`ai-session-metadata.json`、废纸篓与 WebView2 缓存；AI 对话原文由 Pi JSONL 管理，项目文件树永久忽略 `.pi/`；PDF 批注在目标目录下的 `.folio/annotations.json`；标签库为项目下的 `.folio/metadata.sqlite3`（默认空间在 `~/.Folio/.folio/`，独立文件在应用数据目录 `loose-metadata.sqlite3`）；粘贴图片保存在文档目录 `images/<文档名>/`；文档源文件与编译 PDF 均留在项目本地目录，不改动用户文件结构。
- **隐私**：划词翻译会把选中文本发送给 Google（或其百度回退）服务；AI 助手会把输入、附件与选区发送到所配置的网关及启用的联网搜索服务，Exa/千帆凭证只经私有环境变量传给宿主。启用前请确认这符合你对相关内容的处理要求。
- **参考文档**：Markdown 写作提示见 [FOLIO_MARKDOWN.md](FOLIO_MARKDOWN.md)，数学公式支持范围见 [LATEX_SUPPORT_BOUNDARIES.md](LATEX_SUPPORT_BOUNDARIES.md) 与 [LATEX_EXTENSION_GUIDE.md](LATEX_EXTENSION_GUIDE.md)；AI 运行时与工具见 [docs/ai-runtime.md](docs/ai-runtime.md)，宿主扩展见 [resources/pi-host/README.md](resources/pi-host/README.md)。

