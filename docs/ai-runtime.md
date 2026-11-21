# AI runtime and document tools

Folio uses one Node SDK host per application process. Each conversation owns a separate Pi `AgentSession`, `SessionManager`, event subscription, pending dialog queue, and GPUI conversation entity. Switching projects or closing a tab does not abort a run. Closing Folio terminates the host and its subprocess tree through the Windows job object.

## Setup and packaging

```pwsh
pwsh -File scripts/setup-ai.ps1 -InstallBrowser
pwsh -File scripts/install-document-renderers.ps1
pwsh -File scripts/package.ps1
```

The SDK host requires system Node >=22.19.0. `setup-ai.ps1` installs the npm lockfile, creates an isolated Python 3.12 environment, and synchronizes `requirements.lock`. `-InstallBrowser` installs the Patchright Chromium used by Scrapling's explicit dynamic mode. Browser downloads stay in the user's browser cache and are not copied into releases. The package contains the host, npm dependencies, and isolated fetch environment; the system Node and the Python base interpreter used by the virtual environment must remain installed.

In AI settings, configure the Magpie Base URL, title model ID, Exa API Key, and Baidu Qianfan API Key. The initial title model is `workbuddy-ai/hy4flash-free`. Search credentials are masked and passed privately to the host; tool arguments contain no credentials. Machine-specific settings and MCP tokens are never part of the repository.

## Session ownership and persistence

- Project identity is the canonical project-root path, normalized for Windows. Session discovery and all commands enforce that identity.
- Pi JSONL remains authoritative for conversations and names. Folio stores pin state and title provenance in `ai-session-metadata.json`, and each project's open tabs and active tab in `ai-tabs.json`, under its user-data directory.
- Restored tabs load their conversation only when activated. Empty sessions have no JSONL and are removed from the restored layout. Interrupted tasks are never automatically resubmitted.
- New tabs receive their identity immediately and become active before SDK initialization. The input is focused after mounting; model discovery runs when the view attaches. Tabs compress and overlap inside the available header width, with the new-tab button directly after the last tab.
- Closing a running tab releases its foreground retention while keeping its conversation entity and host subscription alive until the task settles. Reopening retains it again. Closed, settled sessions are disposed and can later be loaded from JSONL.
- Manual names take precedence over pending automatic title requests. The first plain user message triggers one separate title request; attachment bodies and quoted context are excluded. A failed title request leaves the preview available and the user can rename the session manually.
- Running-session deletion requires confirmation, resolves outstanding dialogs, aborts the run, disposes its runtime, and then removes history and Folio metadata. Closing a tab is a distinct operation.

## Host protocol

`resources/pi-host/host.mjs` accepts one JSON command per stdin line and writes one JSON response/event per stdout line. Diagnostics go to stderr. Requests carry `requestId`, `routeId`, `cwd`, `projectKey`, and, for conversation operations, `sessionId`.

Project commands are `list_sessions`, `create_session`, and `configure`. Conversation commands include `snapshot`, `prompt`, `abort`, `retain`, `release`, `set_session_name`, `delete_session`, `set_pinned`, model/thinking operations, compaction, and extension UI responses. Responses echo request and route IDs and contain either `success: true, data` or `success: false, error`.

Events carry project/session identity and a monotonically increasing sequence within the live session runtime. A `snapshot` includes its sequence boundary and current messages, including an in-progress assistant message. Conversation views discard older events after applying that boundary. `agent_settled`, rather than `agent_end`, marks completion. Outstanding dialogs are scoped to the conversation and displayed one at a time.

## Web and OCR tools

Search tools expose the full capabilities of the supplied WisePen provider adapters separately, so switching providers cannot silently discard filters:

- `exa_search`: `query`, `mode` (`web` or `academic`), `focus`, `recency`, `include_domains`, `exclude_domains`, and `max_results` (1–20, default 10). Academic mode selects research papers; focus guides highlight extraction. Recency maps to a publication-date lower bound.
- `baidu_qianfan_search`: `query`, `recency`, `include_domains`, and `max_results` (1–20, default 10). The `day` filter uses the current Beijing calendar date. Academic mode, focus, and excluded domains are not supported.

The search tool descriptions prefer Exa and direct the assistant to use Baidu after failure or no results, or directly when Chinese sources are particularly important. This is an explicit tool choice, not a hidden automatic fallback. Domain restrictions and academic requirements must be preserved; cancellation must not start another search. Both tools return provider, query, actual mode, and ranked `candidates` with title, URL, publication date, `evidences`, and provider metadata. Duplicate URLs are removed; URL-less candidates remain distinct. Provider credentials remain outside tool arguments and results.

Exa requests resolve the current Windows system proxy through PowerShell 7/.NET, without inheriting proxy environment overrides, and explicitly use an Undici proxy dispatcher. Missing proxy configuration is an error; neither proxy failure nor `NO_PROXY` permits direct Exa connections. Proxy lookup and requests support cancellation. The dispatcher is disposed after each request. Baidu and the model gateway keep their existing network behavior.

Network tools distinguish `status: ok`, `empty`, `error`, and `cancelled`. Errors include a short `error.type`, `message`, `retryable`, and an HTTP status when available. Connection failures, timeouts, authentication/permission failures, exhausted quota, rate limits, remote errors, malformed responses, local dependency/process errors and parsing errors have separate categories. Empty results do not set Pi's `isError`; failures and cancellation do. Python tracebacks remain in host diagnostics, not model-visible results. The GPUI tool row shows compact arguments; running calls show live input JSON, and completed calls can be expanded to inspect inputs and outputs, including file-edit inputs alongside the diff.

`web_fetch` reads HTTP response bytes before constructing a Scrapling HTML response, so extensionless and redirected PDF URLs are classified before HTML parsing. HTML uses Scrapling Markdown extraction; `mode: dynamic` starts a temporary browser explicitly. PDF uses pdf-inspector and accepts one-based page selections. The full Markdown artifact remains on disk, while the model-facing preview is limited to 30,000 characters and declares truncation.

Scanned or mixed PDFs return `pdf_path`, `pdf_type`, and one-based `needs_ocr_pages`. Continue with the independent MCP server `paddleocr` (model `PaddleOCR-VL-1.6`), using its `paddleocr_vl` tool; it is not an embedded local OCR model or an automatic hidden fetch dependency. MCP configuration is merged into the user's Pi `mcp.json`, preserving other servers. Server IDs must follow Pi's naming rules; the model version belongs in the environment configuration, not in a server ID containing a dot.

## Folio annotations

`folio_annotations_list({path})` reads Folio annotations. `folio_annotation_create({path, quote, comment, page?, occurrence?})` adds a yellow highlight and comment. PDF pages and match occurrences are one-based at the tool boundary; the existing sidecar format remains zero-based. Repeated matches return candidates unless the caller specifies an occurrence. Repeating the same annotation is idempotent; a different existing comment is never overwritten.

The SDK sends a project/session-scoped `annotation_request`; Rust prepares Markdown with the editor model or PDF with the viewer's native text layer, then commits on the GPUI thread. It checks that the originating project is still active, the target stays inside its canonical root, and the document has not changed during preparation. Markdown uses the live buffer when open. Tool requests never activate a document tab or focus an editor. Cancellation retires pending work before its commit.

Markdown highlights use the normal serializer and save path; PDF annotations only update the adjacent `.folio/annotations.json`. The store merges preserve other files' annotations and malformed stores fail instead of being treated as empty. Source and sidecar are separate files: if source saving succeeds but sidecar saving fails, the tool explicitly reports that the highlight was saved without its comment.

## Draw.io tools

`folio_drawio_read({path})` reads a project `.drawio` file. `folio_drawio_create({path, xml})` creates a new native Draw.io XML file and refuses to overwrite an existing path. `folio_drawio_update({path, xml})` replaces an existing diagram with the complete XML document. Write operations validate that the root element is `mxfile`; updates to an open Draw.io pane are picked up by the existing file watcher and Draw.io reload path.

The first version supports single Markdown text blocks and text-layer PDF pages. It excludes scans/OCR coordinates, picture/formula regions, cross-block anchors, native PDF/Word comment export, and automatic anchor repair. The small `folio-annotations` skill lives in the Magpie shared library; standalone packages carry a bundled copy, loaded when the Pi user skill is absent. Workflow guidance stays in that skill and tool descriptions, not the system prompt.

Focused checks: `cargo test --release --test annotations annotation_` and `node --test --test-force-exit --test-name-pattern annotation resources/pi-host/test/host.test.mjs resources/pi-host/test/annotation-tools.test.mjs`.

## Shared document renderers

Versioned additions live in `resources/pi-skills`. The installer copies scripts into the existing Magpie `pdf`, `docx`, and `pptx` skills and adds the Windows instructions once. The PDF skill owns the renderer; Word and PowerPoint entrypoints use it through their resolved shared-library locations.

```pwsh
python scripts/render_pages.py C:/path/document.docx --pages '1,3-5' --output C:/path/preview --dpi 144
```

The same interface works for PDF and PPTX. Page numbers are one-based. Word uses a read-only document and a temporary native PDF export. PowerPoint exports selected slides directly. The JSON result maps page numbers to absolute PNG paths, which the agent must open to inspect visual content. Dependencies are `pywin32`, `pypdfium2`, and `Pillow`, with native Microsoft Office for Office formats.

## Focused validation

`npm test --prefix resources/pi-host` exercises the public JSONL protocol with a controlled OpenAI-compatible server: simultaneous independent streams, live snapshots, project isolation, close/reopen retention, deletion protection, manual-title precedence, pinning, deletion, and stable request prefixes with runtime time context. Separate tests cover provider-specific search options, evidence/metadata preservation, and cancellation. No fixed sleeps are used.

## Folio identity and current time

Folio appends a fixed system policy to the existing Pi resources, preserving project instructions, skills and tool rules. It identifies the assistant as Folio's AI assistant, describes its workspace role and explains runtime timestamps. Search selection, provider limits and source citations belong to tool descriptions rather than this system policy.

The `before_agent_start` extension appends one hidden `folio_runtime_context` message after the user input for each run. It records local date/time, the system's IANA time zone and UTC. The message persists in Pi JSONL and is reused unchanged throughout tool rounds and future history; the next run appends a new snapshot. Folio does not rewrite timestamps in the system prompt or earlier messages, preserving the existing request prefix for provider caching. Actual cache hits depend on the selected model/gateway. Long-running tasks needing an exact live clock must read it through a tool.

## Workspace controls

The shortcut settings include AI panel toggle (default `Ctrl+Shift+A`) and terminal toggle (default Ctrl+backtick). Both work throughout the workspace, including terminal focus, and focus the opened panel. Escape closes an open AI session-history popup before its search/rename input handles the key, then returns focus to the conversation.

Completed answers have an export-to-note button beside Copy, using the bundled `NotebookPen` SVG. Export writes the same Markdown text as Copy to the source conversation's project root, using its session title and a timestamp as the filename, and opens the saved note. File creation refuses to overwrite an existing path; errors appear in the conversation toast.

The conversation's left edge includes a compact message navigator. Longer ticks represent user messages and shorter ticks represent assistant runs; consecutive assistant messages share a target, matching the rendered conversation. Hovering previews visible prose, excluding thinking and tool logs. Clicking jumps to the corresponding scroll row and pauses following new output. The reading-position marker uses the live scroll handle during paint, including scrollbar drags. The rail compresses to fit the message viewport as the conversation grows, while normal scrolling remains available on the right.

Real-machine verification also covers the Magpie title model, both search providers, static and dynamic HTML, native-text and scanned PDFs, PaddleOCR Markdown output, and selected Word/PowerPoint/PDF page rendering. GPUI appearance and hands-on interaction remain subject to user acceptance.
