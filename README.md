# Folio GPUI prototype

This isolated native app evaluates GPUI Kit's editor component with three independent LaTeX documents mounted at once. The original Electron project remains untouched.

## Run

Requires Rust and a native Windows build toolchain supported by GPUI.

```powershell
cargo run --manifest-path gpui-prototype/Cargo.toml
```

The prototype is intentionally limited to editor rendering and input. It does not read or write project files, compile TeX, or render PDFs, so it does not yet answer whether a full rewrite would improve the current multi-preview workload.

## Evaluation boundary

- The editor state is owned by GPUI and its text component; there is no Chromium renderer or DOM per editor.
- Each pane stays mounted with its own buffer to exercise concurrent editor memory and redraw behavior.
- LaTeX highlighting uses GPUI Kit's Syntect integration; this does not yet reproduce CodeMirror's editing behavior or settings.
- PDF rendering remains a separate decision. A replacement must preserve the current PDF text layer, selection, outline, navigation, zoom, and failure behavior before memory comparisons are meaningful.

The next useful gate is an isolated PDF renderer experiment and a repeatable comparison against the existing app using the same documents, page count, zoom, and number of visible panes. Until that exists, this project is evidence about the native editor layer only.
