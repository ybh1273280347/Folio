# Upstream provenance

- Project: [Modolet/gpui_xterm](https://github.com/Modolet/gpui_xterm)
- Pinned source commit: `eda177f30591e1a063e69234a6e2fb1e4edac446`
- Upstream package version: `0.3.0`
- License: MIT; see [LICENSE](LICENSE).

This directory vendors the pinned upstream source. Local changes are kept in
`src/` and adapt its terminal protocol handling and rendering to Folio:

- forward Alacritty PTY writes and answer color, size, and clipboard requests;
- use the configured palette for OSC color replies and selection rendering;
- bound and batch PTY output while continuing to process hidden entities;
- route terminal paste and key input, report supported focus changes, and
  disable Kitty keyboard negotiation until a matching encoder is implemented;
- keep host dimensions synchronized with render-time font measurement and
  expose windowless title and exit observers for hidden panes.
- route mouse buttons, movement and wheel events using the negotiated mouse
  encoding; keep Shift selection and alternate-screen scrolling available.
- leave printable key events unconsumed so Windows can generate WM_CHAR and
  deliver English text through the same input handler as IME commits.

The original upstream license and package metadata remain alongside these
local patches.
