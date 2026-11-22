# Folio RAG 知识库与右侧栏交接手册

本文用于在新会话中直接接手 **本地知识库（RAG）、PDF 解析与分块、右侧栏知识库面板** 的后续重构。内容仅记录当前代码库已落地的模块边界、代码位置、关键契约与验证基线。

## 1. 当前仓库基线

- **当前分支**：`main`
- **HEAD**：`6ba8273`（`Merge branch 'feat/rag-dedup-derived-pdfs'`）
- **桌面打包状态**：上次桌面端 `dist/Folio/Folio.exe` 发布于 `7f7b0b6` 之后；最新合入的 `6ba8273`（衍生 PDF 协议指纹过滤）**尚未重新执行 `scripts/package.ps1` 打包**。

近期已合入 `main` 的相关阶段：

```text
6ba8273 Merge branch 'feat/rag-dedup-derived-pdfs'          # 衍生 PDF 仅按协议指纹过滤（不看文件名）
7f7b0b6 Merge branch 'feat/kb-book-cards-and-jump'          # 人类纯关键词检索 + 书本卡片 + 1.4s 跳转高亮
e6bbdb8 merge: 合入阶段1 PaddleOCR-VL 扫描 PDF 精修管线       # 移除 MinerU，接入 PaddleOCR-VL-1.6 Jobs API
5d7be08 merge: 合入知识库语义状态检测与跳转显示修复
```

---

## 2. 核心设计原则与运行链路

```text
[人类右侧栏面板] ──(纯 Lex / 不重排 / 不唤醒 llama-server)──┐
                                                             ▼
[AI Agent 工具]  ──(QMD searches(lex/vec) + intent + rerank)──▶ src/rag 引擎 ──▶ .folio/rag.sqlite3
                                                             ▲                  (FTS5 + sqlite-vec)
[PDF 提取两级]
  ├─ Fast（日常默认）：pdf-inspector 按页抽 Markdown（失败退回 gpui_pdf），按 token 预算在页内切分
  └─ Accurate（显式触发）：PaddleOCR-VL-1.6 → .folio/pdf-cache/<sha256>/full.md + pages.json → 按标题跨页二次分块
```

1. **人机分流**：
   - **人类在右侧栏检索**：只发单条 `SubSearchType::Lex` + `rerank: Some(false)`，毫秒级查 FTS5 与编辑器未保存草稿（`DirtyDocument`），不拉起本地 `llama-server`。
   - **AI 工具检索（`folio_kb_search`）**：接收多条 `searches`（`lex`/`vec`）与 `intent`，走 RRF 融合 + 位置感知重排，按需拉起本地 embedding / rerank 服务（默认端口 `18101` / `18102`）。
2. **衍生 PDF 去重仅看协议指纹**：
   - `collect_eligible_paths` 仅按扩展名收集，不再因同目录有 `.tex`/`.ltx` 同名跳过 PDF。
   - `compile_tex`、Markdown 导出、图片/SVG 导出分别写入 `.folio/derived-pdfs.json`（PDF 字节 SHA-256）及 PDF 内嵌 `krilla` 元数据（`keywords = ["folio-derived"]`）。
   - `sync_project` 仅对命中上述指纹的 PDF 标 `status = "derived"`（键 `derived:<sha256>`，0 分块），与文件名无关。

---

## 3. 代码位置与模块职责地图

### A. RAG 核心引擎（`src/rag/`）

| 文件 | 关键符号 / 入口 | 职责 |
| :--- | :--- | :--- |
| [src/rag/mod.rs](file:///d:/projects/my_projects/Latex/gpui-prototype/src/rag/mod.rs) | `SubSearchType`, `SubSearch`, `SearchRequest`, `AccuratePdfMode`, `SearchHit`, `StatusReport`, `ReindexReport`, `collect_eligible_paths`, `record_derived_pdf`, `load_derived_pdf_hashes`, `has_folio_pdf_fingerprint` | 对外协议类型、支持扩展名筛选（`md`/`txt`/`tex`/`ltx`/`bib`/`pdf`）以及 `.folio/derived-pdfs.json` + PDF 内嵌指纹读写与校验 |
| [src/rag/chunker.rs](file:///d:/projects/my_projects/Latex/gpui-prototype/src/rag/chunker.rs) | `ChunkDraft`, `ExtractedDocument`, `chunk_text_document`, `extract_and_chunk_pdf`, `chunk_annotations`, `split_token_chunks` | 文本与 PDF 共用 Qwen3 token 预算，保留行号与页码锚点 |
| [src/rag/ocr.rs](file:///d:/projects/my_projects/Latex/gpui-prototype/src/rag/ocr.rs) | `PageAnchor`, `resolve_ocr_token`, `load_cached_pdf`, `upgrade_pdfs`, `build_multipart_body`, `parse_paddleocr_jsonl_to_cache`, `match_chunk_page` | PaddleOCR-VL-1.6 异步任务客户端（`paddleocr.aistudio-app.com/api/v2/ocr/jobs`）、Token 自动解析（设置 → 环境变量 → `~/.pi/agent/mcp.json`）及 `.folio/pdf-cache/<sha256>/` 缓存读写 |
| [src/rag/pipeline.rs](file:///d:/projects/my_projects/Latex/gpui-prototype/src/rag/pipeline.rs) | `SyncOptions`, `sync_project`, `SearchServices`, `search_project`, `position_aware_blend_scores`, `clean_heading_text`, `clean_markdown_snippet`, `build_snippet` | 增量同步、批注同步、衍生 PDF 跳过、RRF 融合（`RRF_K = 60.0`, `MAX_CHUNKS_PER_FILE = 4`）、重排打分及摘要清洗截断（`SNIPPET_CHARS = 120`） |
| [src/rag/store.rs](file:///d:/projects/my_projects/Latex/gpui-prototype/src/rag/store.rs) | `RagStore`, `DocumentMeta`, `StoreStats`, `sync` 表操作 (`upsert_document`, `deactivate_missing`, `search_fts`, `search_vec`, `pdf_upgrade_candidates`, `ocr_counts`, `stats`) | SQLite WAL + FTS5 trigram + `sqlite-vec` 持久化层；统计与 OCR 候选均排除 `status = 'derived'` |
| [src/rag/client.rs](file:///d:/projects/my_projects/Latex/gpui-prototype/src/rag/client.rs) | `EmbeddingClient`, `RerankClient`, `reap_idle_servers`, `shutdown_managed_servers` | 本地 `llama-server` 按需拉起、空闲回收与 HTTP `/v1/embeddings` / `/v1/rerank` 调用 |

### B. 知识库面板与跳转高亮（UI / Workspace）

| 文件 | 关键符号 / 入口 | 职责 |
| :--- | :--- | :--- |
| [src/workspace/panels.rs](file:///d:/projects/my_projects/Latex/gpui-prototype/src/workspace/panels.rs) | `RightPanelView::KnowledgeBase`, `RightPanel` | 右侧栏视图枚举与面板状态挂载（`RightPanel.kb`） |
| [src/workspace/panels/hub.rs](file:///d:/projects/my_projects/Latex/gpui-prototype/src/workspace/panels/hub.rs) | 右侧栏 Hub 卡片与导航入口 | 从右侧栏 Hub 打开知识库 |
| [src/workspace/panels/kb.rs](file:///d:/projects/my_projects/Latex/gpui-prototype/src/workspace/panels/kb.rs) | `KbFilter`, `KbPanelState`, `render_kb_active_body`, `render_kb_search_results`, `render_kb_ocr_card` | 搜索框、全部/笔记/PDF 筛选 chip、`N 条 · M 篇` 计数、旋转同步/搜索图标、书本式文件分组卡片（每文件默认 2 条可展开，宽栏双列网格）、扫描件显式精修卡片 |
| [src/i18n/right_panel.rs](file:///d:/projects/my_projects/Latex/gpui-prototype/src/i18n/right_panel.rs) | `KB_FILTER_*`, `kb_search_summary`, `kb_more_hits` 等 | 知识库面板中英文双语文案 |
| [src/workspace/documents.rs](file:///d:/projects/my_projects/Latex/gpui-prototype/src/workspace/documents.rs) | `open_kb_location`, `jump_to_document_line`, `OpenDocument::{line_highlight, line_highlight_timer}` | 统一路由搜索命中跳转到 PDF / Markdown / 纯文本并维护纯文本 1400ms 行高亮 |
| [src/workspace.rs](file:///d:/projects/my_projects/Latex/gpui-prototype/src/workspace.rs) | `line_byte_range` | 行号到字节范围转换 helper |
| [src/markdown/pane.rs](file:///d:/projects/my_projects/Latex/gpui-prototype/src/markdown/pane.rs) | `jump_to_search_hit` | Markdown 源码模式跳行并触发 1400ms 高亮；可视化模式优先精确引用短语高亮或大纲锚点跳转 |
| [src/pdf.rs](file:///d:/projects/my_projects/Latex/gpui-prototype/src/pdf.rs) | `PdfPane::jump_to_page`, `pending_jump_page` | 面板打开前暂存目标页，打开后委托 `PdfView::flash_to_page` |
| [vendor/gpui-pdf/src/lib.rs](file:///d:/projects/my_projects/Latex/gpui-prototype/vendor/gpui-pdf/src/lib.rs) | `PdfView::flash_to_page`, `pending_reveal` | 滚至目标页顶部并绘制 1400ms 闪烁边框（已登记于 [vendor/FOLIO-PATCHES.md](file:///d:/projects/my_projects/Latex/gpui-prototype/vendor/FOLIO-PATCHES.md)） |

### C. Workspace 工具桥、AI 宿主与派生指纹生产者

| 文件 | 关键符号 / 入口 | 职责 |
| :--- | :--- | :--- |
| [src/workspace/tools/rag.rs](file:///d:/projects/my_projects/Latex/gpui-prototype/src/workspace/tools/rag.rs) | `RagSyncState`, `submit_kb_search`, `open_kb_result`, `start_rag_request`, `trigger_rag_sync_now`, `schedule_rag_sync`, `kb_badge` | 后台防抖增量同步（2.5s）、采集编辑器未保存 `DirtyDocument`、处理人类 UI 搜索与 AI `rag` 请求 |
| [resources/pi-host/tools/workspace/rag-tools.mjs](file:///d:/projects/my_projects/Latex/gpui-prototype/resources/pi-host/tools/workspace/rag-tools.mjs) | `folio_kb_search`, `folio_kb_status`, `folio_kb_reindex` | Node/Pi 宿主工具定义与参数 schema |
| [resources/pi-host/test/rag-tools.test.mjs](file:///d:/projects/my_projects/Latex/gpui-prototype/resources/pi-host/test/rag-tools.test.mjs) | 2 条 Node 参数转发与取消映射单测 | 验证 AI 工具参数未二次包装与取消错误语义 |
| [src/build/compiler.rs](file:///d:/projects/my_projects/Latex/gpui-prototype/src/build/compiler.rs) | `compile_tex` | LaTeX 编译输出后登记 `origin = "tex"` 到 `.folio/derived-pdfs.json` |
| [src/markdown/pdf.rs](file:///d:/projects/my_projects/Latex/gpui-prototype/src/markdown/pdf.rs) | `ExportRequest`, `Document::set_metadata` | Markdown 导出写入 `folio-derived` 元数据并登记 `origin = "markdown"` |
| [src/image/pdf.rs](file:///d:/projects/my_projects/Latex/gpui-prototype/src/image/pdf.rs) | `export_image_to_pdf` | SVG/位图导出写入 `folio-derived` 元数据并登记 `origin = "image"` |
| [src/workspace/build.rs](file:///d:/projects/my_projects/Latex/gpui-prototype/src/workspace/build.rs) | `start_build` | 向 Markdown / Image PDF 导出传入项目根目录与源路径 |
| [src/model.rs](file:///d:/projects/my_projects/Latex/gpui-prototype/src/model.rs) | `RagConfig`（第 367 行） | 知识库 URL/模型路径、空闲回收秒数与 `paddleocr_token` 配置 |
| [src/preferences.rs](file:///d:/projects/my_projects/Latex/gpui-prototype/src/preferences.rs) | 知识库与 OCR 设置表单 | 模型端口、GGUF 路径与 PaddleOCR Token 输入项 |

---

## 4. 数据布局与分块现状（后续重构关注点）

### 项目内持久化目录（`.folio/`）
- `.folio/rag.sqlite3`：核心检索库。`contents` 表按 `content_hash` 存原文与 `parse_tier`（`"source"` / `"fast_pdf"` / `"accurate"` / `"annotation"` / `"derived"`）；`documents` 存相对路径、修改时间、`status`（`"indexed"` / `"needs_ocr"` / `"derived"`）；`chunks` + `chunks_fts` + `chunks_vec` 存分块与向量。
- `.folio/pdf-cache/<sha256>/`：`PaddleOCR-VL-1.6` 解析产物（`full.md` 与 `pages.json` 锚点数组）。
- `.folio/derived-pdfs.json`：Folio 编译/导出产物 SHA-256 清单。

### Token 分块与索引迁移

- 正常目标为 384 tokens，硬上限为 480 tokens，均含送入 embedding 的标题前缀。块间重叠至多约 56 tokens，结构化文本优先在行与完整块之间断开。单行或保护块超出上限时，从原文的 UTF-8 边界拆分。
- 使用 [Qwen3-Embedding-0.6B 官方 tokenizer](https://huggingface.co/Qwen/Qwen3-Embedding-0.6B/blob/b22da495047858cce924d27d76261e96be6febc0/tokenizer.json)，按 Apache-2.0 随程序编译嵌入。运行时直接本地分词。
- `chunker_version` 从 1 提升到 2。项目下次同步会重建分块、FTS、向量与重排派生缓存，OCR 分页文件继续复用。

### PDF 解析与分块边界

1. **Fast 路径逐页独立分块**：优先选择页内段落边界，保留每个分块的原页码。短页仍分别成块。
2. **Accurate 路径与 Fast 路径策略分裂**：Accurate 路径先把所有页拼成 `full.md`，再走 `chunk_structured_lines` 跨页按 `#` 标题二次分块并用 `pages.json` 锚点回填 `page_num`；Fast 路径尚未使用统一的后处理策略。
3. **摘要清洗只发生在展示/响应阶段**：`chunks` 表仍存带原始 `[heading]\n...` 与 Markdown/LaTeX 的块（便于保留上下文与命中高亮还原），展示摘要时在 `build_snippet` 中即时剥离与截断；若改为双列/结构化存储或调整分块策略，需同步核对 `clean_markdown_snippet` 与 Markdown/PDF 跳转定位逻辑。

---

## 5. 验证命令与协作约定

- **聚焦编译与测试**：
  ```powershell
  cargo check --locked --bin folio-gpui-prototype --tests -j 2
  cargo test --locked --bin folio-gpui-prototype rag:: -j 2
  cargo test --locked --bin folio-gpui-prototype image::pdf::tests -j 2
  cargo test --locked --bin folio-gpui-prototype build:: -j 2
  cargo test --locked --test appearance -j 2
  node --test --test-force-exit resources/pi-host/test/rag-tools.test.mjs
  ```
- **打包发布桌面端**：`pwsh -NoProfile -ExecutionPolicy Bypass -File .\scripts\package.ps1 -Launch`
- **仓库规则提醒**：遵循 [AGENTS.md](file:///d:/projects/my_projects/Latex/gpui-prototype/AGENTS.md)，每次重构从最新 `main` 开分支，验证通过后 `--no-ff` 合回 `main`；修改 `vendor/` 下代码同步登记至 [vendor/FOLIO-PATCHES.md](file:///d:/projects/my_projects/Latex/gpui-prototype/vendor/FOLIO-PATCHES.md)。
