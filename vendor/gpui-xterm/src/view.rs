use crate::colors::ColorPalette;
use crate::event::{GpuiEventProxy, TerminalEvent};
use crate::input::keystroke_to_bytes;
use crate::mouse::pixel_to_cell;
use crate::render::TerminalRenderer;
use crate::terminal::TerminalState;
use alacritty_terminal::event::WindowSize;
use alacritty_terminal::grid::Scroll;
use alacritty_terminal::index::Side;
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::TermMode;
use gpui_kit::component::menu::{ContextMenuExt, PopupMenu, PopupMenuItem};
use gpui_kit::{Edges, ElementInputHandler, EntityInputHandler, UTF16Selection, *};
use std::collections::VecDeque;
use std::io::{Cursor, Read, Write};
use std::sync::Arc;
use std::sync::mpsc;
use std::thread;

const CONTEXT: &str = "GpuiXterm";
const OUTPUT_QUEUE_CHUNKS: usize = 64;
const OUTPUT_BATCH_BYTES: usize = 64 * 1024;
const PENDING_UI_EVENT_LIMIT: usize = 64;

actions!(gpui_xterm, [SendTab, SendBacktab]);

#[derive(Clone, Debug)]
pub struct TerminalConfig {
    pub cols: usize,

    pub rows: usize,

    pub font_family: String,

    pub font_size: Pixels,

    pub scrollback: usize,

    pub line_height_multiplier: f32,

    pub padding: Edges<Pixels>,

    pub minimum_contrast: f32,

    pub colors: ColorPalette,
}

impl Default for TerminalConfig {
    fn default() -> Self {
        Self {
            cols: 80,
            rows: 24,
            font_family: "monospace".into(),
            font_size: px(14.0),
            scrollback: 10000,
            line_height_multiplier: 1.0,
            padding: Edges::all(px(0.0)),
            minimum_contrast: 4.5,
            colors: ColorPalette::default(),
        }
    }
}

pub type ResizeCallback = Box<dyn Fn(usize, usize) + Send + Sync>;

pub type KeyHandler = Box<dyn Fn(&KeyDownEvent) -> bool + Send + Sync>;

pub type BellCallback = Box<dyn Fn(&mut Window, &mut Context<TerminalView>)>;

pub type TitleCallback = Box<dyn Fn(&mut Window, &mut Context<TerminalView>, &str)>;

pub type ClipboardStoreCallback = Box<dyn Fn(&mut Window, &mut Context<TerminalView>, &str)>;

pub type ExitCallback = Box<dyn Fn(&mut Window, &mut Context<TerminalView>)>;

pub type TitleObserver = dyn Fn(&str) + Send + Sync;

pub type ExitObserver = dyn Fn() + Send + Sync;

#[derive(Clone, Copy, Debug, Default)]
struct LayoutMetrics {
    origin: Point<Pixels>,
    cell_width: Pixels,
    cell_height: Pixels,
    cols: usize,
    rows: usize,
    scale_factor: f32,
}

pub struct TerminalView {
    state: TerminalState,

    renderer: TerminalRenderer,

    focus_handle: FocusHandle,

    stdin_writer: Arc<parking_lot::Mutex<Box<dyn Write + Send>>>,

    event_rx: mpsc::Receiver<TerminalEvent>,

    pending_ui_events: VecDeque<TerminalEvent>,

    config: TerminalConfig,

    #[allow(dead_code)]
    _reader_task: Task<()>,

    resize_callback: Option<Arc<ResizeCallback>>,

    key_handler: Option<Arc<KeyHandler>>,

    bell_callback: Option<BellCallback>,

    title_callback: Option<TitleCallback>,

    clipboard_store_callback: Option<ClipboardStoreCallback>,

    exit_callback: Option<ExitCallback>,

    title_observer: Option<Arc<TitleObserver>>,

    exit_observer: Option<Arc<ExitObserver>>,

    focus_subscriptions: Option<(Subscription, Subscription)>,

    layout_metrics: Arc<parking_lot::Mutex<LayoutMetrics>>,

    dragging_selection: bool,

    ime_text: String,
    ime_marked_range: Option<std::ops::Range<usize>>,
}

fn should_defer_text_input_to_ime(event: &KeyDownEvent) -> bool {
    event.keystroke.key_char.is_some()
        && !event.keystroke.modifiers.control
        && !event.keystroke.modifiers.alt
        && !event.keystroke.modifiers.platform
        && !event.keystroke.modifiers.function
}

fn protocol_dimension(value: usize) -> u16 {
    value.min(u16::MAX as usize) as u16
}

fn protocol_pixels(value: Pixels, scale_factor: f32) -> u16 {
    let logical_pixels: f32 = value.into();
    (logical_pixels * scale_factor)
        .round()
        .clamp(0.0, u16::MAX as f32) as u16
}

fn normalize_paste_text(text: &str) -> String {
    let mut normalized = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' {
            continue;
        }

        match ch {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                normalized.push('\r');
            }
            '\n' => normalized.push('\r'),
            _ => normalized.push(ch),
        }
    }

    normalized
}

impl TerminalView {
    pub fn new<W, R>(
        stdin_writer: W,
        stdout_reader: R,
        config: TerminalConfig,
        cx: &mut Context<Self>,
    ) -> Self
    where
        W: Write + Send + 'static,
        R: Read + Send + 'static,
    {
        cx.bind_keys([
            KeyBinding::new("tab", SendTab, Some(CONTEXT)),
            KeyBinding::new("shift-tab", SendBacktab, Some(CONTEXT)),
        ]);

        let (event_tx, event_rx) = mpsc::channel();

        let exit_event_tx = event_tx.clone();

        let event_proxy = GpuiEventProxy::new(event_tx);

        let state = TerminalState::new(config.cols, config.rows, event_proxy);

        let renderer = TerminalRenderer::new(
            config.font_family.clone(),
            config.font_size,
            config.line_height_multiplier,
            config.colors.clone(),
        )
        .with_minimum_contrast(config.minimum_contrast);

        let focus_handle = cx.focus_handle().tab_stop(false);

        let stdin_writer = Arc::new(parking_lot::Mutex::new(
            Box::new(stdin_writer) as Box<dyn Write + Send>
        ));

        // Backpressure the PTY reader at a fixed memory bound; the UI task drains
        // several adjacent reads together so terminal bursts cause one update.
        let (bytes_tx, bytes_rx) = flume::bounded::<Vec<u8>>(OUTPUT_QUEUE_CHUNKS);

        thread::spawn(move || {
            Self::read_stdout_blocking(stdout_reader, bytes_tx);
        });

        let reader_task = cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            loop {
                let Ok(first_chunk) = bytes_rx.recv_async().await else {
                    let _ = exit_event_tx.send(TerminalEvent::Exit);
                    let _ = this.update(cx, |view, cx: &mut Context<Self>| {
                        view.process_output_events();
                        cx.notify();
                    });
                    break;
                };

                let mut batch = first_chunk;
                while batch.len() < OUTPUT_BATCH_BYTES {
                    match bytes_rx.try_recv() {
                        Ok(chunk) => batch.extend_from_slice(&chunk),
                        Err(_) => break,
                    }
                }

                let result = this.update(cx, |view: &mut Self, cx: &mut Context<Self>| {
                    view.state.process_bytes(&batch);
                    view.process_output_events();
                    cx.notify();
                });
                if result.is_err() {
                    break;
                }
            }
        });

        Self {
            state,
            renderer,
            focus_handle,
            stdin_writer,
            event_rx,
            pending_ui_events: VecDeque::new(),
            config,
            _reader_task: reader_task,
            resize_callback: None,
            key_handler: None,
            bell_callback: None,
            title_callback: None,
            clipboard_store_callback: None,
            exit_callback: None,
            title_observer: None,
            exit_observer: None,
            focus_subscriptions: None,
            layout_metrics: Arc::new(parking_lot::Mutex::new(LayoutMetrics::default())),
            dragging_selection: false,
            ime_text: String::new(),
            ime_marked_range: None,
        }
    }

    pub fn new_virtual(config: TerminalConfig, cx: &mut Context<Self>) -> Self {
        Self::new(std::io::sink(), Cursor::new(Vec::<u8>::new()), config, cx)
    }

    pub fn with_resize_callback(
        mut self,
        callback: impl Fn(usize, usize) + Send + Sync + 'static,
    ) -> Self {
        self.resize_callback = Some(Arc::new(Box::new(callback)));
        self
    }

    pub fn with_key_handler(
        mut self,
        handler: impl Fn(&KeyDownEvent) -> bool + Send + Sync + 'static,
    ) -> Self {
        self.key_handler = Some(Arc::new(Box::new(handler)));
        self
    }

    pub fn with_bell_callback(
        mut self,
        callback: impl Fn(&mut Window, &mut Context<TerminalView>) + 'static,
    ) -> Self {
        self.bell_callback = Some(Box::new(callback));
        self
    }

    pub fn with_title_callback(
        mut self,
        callback: impl Fn(&mut Window, &mut Context<TerminalView>, &str) + 'static,
    ) -> Self {
        self.title_callback = Some(Box::new(callback));
        self
    }

    pub fn with_clipboard_store_callback(
        mut self,
        callback: impl Fn(&mut Window, &mut Context<TerminalView>, &str) + 'static,
    ) -> Self {
        self.clipboard_store_callback = Some(Box::new(callback));
        self
    }

    pub fn with_exit_callback(
        mut self,
        callback: impl Fn(&mut Window, &mut Context<TerminalView>) + 'static,
    ) -> Self {
        self.exit_callback = Some(Box::new(callback));
        self
    }

    /// Observe title changes without a Window so hidden terminal panes keep the host title current.
    pub fn with_title_observer(mut self, callback: impl Fn(&str) + Send + Sync + 'static) -> Self {
        self.title_observer = Some(Arc::new(callback));
        self
    }

    /// Observe PTY exit while hidden; the existing Window callback still runs when the view renders.
    pub fn with_exit_observer(mut self, callback: impl Fn() + Send + Sync + 'static) -> Self {
        self.exit_observer = Some(Arc::new(callback));
        self
    }

    fn read_stdout_blocking<R: Read + Send + 'static>(
        mut stdout_reader: R,
        bytes_tx: flume::Sender<Vec<u8>>,
    ) {
        let mut buffer = [0u8; 4096];

        loop {
            match stdout_reader.read(&mut buffer) {
                Ok(0) => {
                    break;
                }
                Ok(n) => {
                    let bytes = buffer[..n].to_vec();
                    if bytes_tx.send(bytes).is_err() {
                        break;
                    }
                }
                Err(_) => {
                    break;
                }
            }
        }
    }

    fn on_action_send_tab(&mut self, _: &SendTab, _window: &mut Window, cx: &mut Context<Self>) {
        if self.write_input_bytes(b"\t") {
            cx.notify();
        }
    }

    fn on_action_send_backtab(
        &mut self,
        _: &SendBacktab,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.write_input_bytes(b"\x1b[Z") {
            cx.notify();
        }
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let key = event.keystroke.key.to_lowercase();
        let control = event.keystroke.modifiers.control;
        let shift = event.keystroke.modifiers.shift;

        if control && !shift && key == "c" {
            cx.stop_propagation();
            if self.write_input_bytes(b"\x03") {
                cx.notify();
            }
            return;
        }

        if control && shift && key == "c" {
            cx.stop_propagation();
            self.copy_selection();
            return;
        }

        if control && shift && key == "v" {
            cx.stop_propagation();
            if self.paste_from_clipboard() {
                cx.notify();
            }
            return;
        }

        if control && !shift && !event.keystroke.modifiers.alt && key == "v" {
            if let Ok(mut clipboard) = arboard::Clipboard::new() {
                if clipboard.get_image().is_err()
                    && let Ok(text) = clipboard.get_text()
                {
                    cx.stop_propagation();
                    if self.paste_text(&text) {
                        cx.notify();
                    }
                    return;
                }
            }
        }

        if let Some(ref handler) = self.key_handler
            && handler(event)
        {
            cx.stop_propagation();
            return;
        }

        if should_defer_text_input_to_ime(event) {
            // Windows translates an unhandled key into WM_CHAR; consuming it
            // here suppresses ordinary English input. Text and IME commits both
            // reach replace_text_in_range, so never also write these key bytes.
            return;
        }

        // Only protocol keys handled here are consumed; host shortcuts exclude
        // the GpuiXterm focus context so printable text can keep propagating.
        cx.stop_propagation();

        if key == "tab" {
            let wrote = self.write_input_bytes(if event.keystroke.modifiers.shift {
                b"\x1b[Z"
            } else {
                b"\t"
            });
            if wrote {
                cx.notify();
            }
            return;
        }

        if let Some(bytes) = keystroke_to_bytes(&event.keystroke, self.state.mode())
            && self.write_input_bytes(&bytes)
        {
            cx.notify();
        }
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus_handle, cx);
        if !event.modifiers.shift && self.mouse_reporting() {
            cx.stop_propagation();
            self.state.clear_selection();
            if let Some(code) = mouse_button_code(event.button) {
                self.send_mouse(code, event.position, event.modifiers, false);
            }
            cx.notify();
            return;
        }
        if event.button != MouseButton::Left {
            return;
        }
        cx.stop_propagation();
        let selection_type = match event.click_count {
            2 => SelectionType::Semantic,
            3.. => SelectionType::Lines,
            _ => SelectionType::Simple,
        };
        if let Some(point) = self.hit_test_cell(event.position) {
            self.state
                .update_selection(Some(Selection::new(selection_type, point, Side::Left)));
            self.dragging_selection = true;
            cx.notify();
        }
    }

    fn on_mouse_up(&mut self, event: &MouseUpEvent, _window: &mut Window, cx: &mut Context<Self>) {
        // GPUI delivers drops through mouse-up bubbling to the terminal's parent.
        if cx.has_active_drag() {
            return;
        }
        cx.stop_propagation();
        if !self.dragging_selection && !event.modifiers.shift && self.mouse_reporting() {
            if let Some(code) = mouse_button_code(event.button) {
                self.send_mouse(code, event.position, event.modifiers, true);
            }
            return;
        }
        if !self.dragging_selection {
            return;
        }
        if let Some(point) = self.hit_test_cell(event.position) {
            self.update_selection_endpoint(point);
        }
        self.dragging_selection = false;
        let _ = self.copy_selection();
        cx.notify();
    }

    fn on_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if cx.has_active_drag() {
            return;
        }
        let mode = self.state.mode();
        if !self.dragging_selection
            && !event.modifiers.shift
            && (mode.contains(TermMode::MOUSE_MOTION)
                || (mode.contains(TermMode::MOUSE_DRAG) && event.pressed_button.is_some()))
        {
            let code = event
                .pressed_button
                .and_then(mouse_button_code)
                .unwrap_or(3)
                | 32;
            self.send_mouse(code, event.position, event.modifiers, false);
            cx.stop_propagation();
            return;
        }
        if !self.dragging_selection || !event.dragging() {
            return;
        }
        if let Some(point) = self.hit_test_cell(event.position) {
            self.update_selection_endpoint(point);
            cx.notify();
        }
    }

    fn on_scroll(&mut self, event: &ScrollEvent, _window: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();
        let delta = match event.delta {
            ScrollDelta::Lines(delta) => delta.y.round() as i32,
            ScrollDelta::Pixels(delta) => {
                let line_height: f32 = self.renderer.cell_height.into();
                if line_height <= 0.0 {
                    0
                } else {
                    (f32::from(delta.y) / line_height).round() as i32
                }
            }
        };
        if delta != 0 {
            if !event.modifiers.shift && self.mouse_reporting() {
                let code = if delta > 0 { 64 } else { 65 };
                for _ in 0..delta.unsigned_abs().min(12) {
                    self.send_mouse(code, event.position, event.modifiers, false);
                }
                return;
            }
            if !event.modifiers.shift && self.state.mode().contains(TermMode::ALT_SCREEN) {
                if let Some(bytes) =
                    crate::mouse::scroll_report(delta, Default::default(), 0, self.state.mode())
                {
                    self.write_input_bytes(&bytes);
                }
                return;
            }
            self.state.scroll_display(Scroll::Delta(delta));
            cx.notify();
        }
    }

    fn mouse_reporting(&self) -> bool {
        self.state.mode().intersects(
            TermMode::MOUSE_REPORT_CLICK | TermMode::MOUSE_MOTION | TermMode::MOUSE_DRAG,
        )
    }

    /// Mouse reports use viewport cells, while text selection uses scrollback
    /// coordinates. Respect the negotiated encoding rather than always sending SGR.
    fn send_mouse(&self, code: u8, position: Point<Pixels>, modifiers: Modifiers, released: bool) {
        let metrics = *self.layout_metrics.lock();
        let Some(mut point) = self.hit_test_cell(position) else {
            return;
        };
        point.line.0 += self.state.display_offset() as i32;
        if point.line.0 < 0 || point.line.0 as usize >= metrics.rows {
            return;
        }
        let code = code
            | crate::mouse::encode_modifiers(modifiers.shift, modifiers.alt, modifiers.control);
        let col = point.column.0 + 1;
        let row = point.line.0 as usize + 1;
        let mode = self.state.mode();
        let bytes = if mode.contains(TermMode::SGR_MOUSE) {
            format!(
                "\x1b[<{code};{col};{row}{}",
                if released { 'm' } else { 'M' }
            )
            .into_bytes()
        } else {
            let code = if released { (code & 28) | 3 } else { code };
            let mut bytes = b"\x1b[M".to_vec();
            if mode.contains(TermMode::UTF8_MOUSE) {
                let Some(col) = char::from_u32(col as u32 + 32) else {
                    return;
                };
                let Some(row) = char::from_u32(row as u32 + 32) else {
                    return;
                };
                bytes.push(code + 32);
                let mut buffer = [0; 4];
                bytes.extend_from_slice(col.encode_utf8(&mut buffer).as_bytes());
                bytes.extend_from_slice(row.encode_utf8(&mut buffer).as_bytes());
            } else {
                if col > 223 || row > 223 {
                    return;
                }
                bytes.extend_from_slice(&[code + 32, col as u8 + 32, row as u8 + 32]);
            }
            bytes
        };
        self.write_pty_bytes(&bytes);
    }

    fn hit_test_cell(&self, position: Point<Pixels>) -> Option<alacritty_terminal::index::Point> {
        let metrics = *self.layout_metrics.lock();
        if metrics.cell_width <= px(0.0) || metrics.cell_height <= px(0.0) {
            return None;
        }
        if position.x < metrics.origin.x || position.y < metrics.origin.y {
            return None;
        }
        let mut point = pixel_to_cell(
            position,
            metrics.origin,
            metrics.cell_width,
            metrics.cell_height,
        );
        if point.column.0 >= metrics.cols {
            point.column = alacritty_terminal::index::Column(metrics.cols.saturating_sub(1));
        }
        let offset = self.state.display_offset() as i32;
        point.line = alacritty_terminal::index::Line(point.line.0 - offset);
        Some(point)
    }

    fn update_selection_endpoint(&mut self, point: alacritty_terminal::index::Point) {
        self.state.with_term_mut(|term| {
            if let Some(selection) = term.selection.as_mut() {
                selection.update(point, Side::Right);
            }
        });
    }

    fn process_output_events(&mut self) {
        let mut pty_replies = Vec::new();

        while let Ok(event) = self.event_rx.try_recv() {
            match event {
                TerminalEvent::Wakeup => {}
                TerminalEvent::Bell => self.queue_ui_event(TerminalEvent::Bell),
                TerminalEvent::Title(title) => {
                    if let Some(observer) = &self.title_observer {
                        observer(&title);
                    }
                    self.queue_ui_event(TerminalEvent::Title(title));
                }
                TerminalEvent::ClipboardStore(text) => {
                    if let Ok(mut clipboard) = arboard::Clipboard::new() {
                        let _ = clipboard.set_text(text.clone());
                    }
                    self.queue_ui_event(TerminalEvent::ClipboardStore(text));
                }
                TerminalEvent::ClipboardLoad(format) => {
                    let text = arboard::Clipboard::new()
                        .and_then(|mut clipboard| clipboard.get_text())
                        .unwrap_or_default();
                    pty_replies.extend_from_slice(format(&text).as_bytes());
                }
                TerminalEvent::PtyWrite(text) => {
                    pty_replies.extend_from_slice(text.as_bytes());
                }
                TerminalEvent::ColorRequest(index, format) => {
                    if let Some(rgb) = self.state.with_term(|term| {
                        self.config.colors.color_for_request(index, term.colors())
                    }) {
                        pty_replies.extend_from_slice(format(rgb).as_bytes());
                    }
                }
                TerminalEvent::TextAreaSizeRequest(format) => {
                    let metrics = *self.layout_metrics.lock();
                    let cell_width = if metrics.cell_width > px(0.0) {
                        metrics.cell_width
                    } else {
                        self.renderer.cell_width
                    };
                    let cell_height = if metrics.cell_height > px(0.0) {
                        metrics.cell_height
                    } else {
                        self.renderer.cell_height
                    };
                    let (cols, rows) = if metrics.cols > 0 && metrics.rows > 0 {
                        (metrics.cols, metrics.rows)
                    } else {
                        (self.state.cols(), self.state.rows())
                    };
                    let scale_factor = if metrics.scale_factor > 0.0 {
                        metrics.scale_factor
                    } else {
                        1.0
                    };
                    let size = WindowSize {
                        num_lines: protocol_dimension(rows),
                        num_cols: protocol_dimension(cols),
                        // GPUI lays out in logical pixels; Alacritty's window-size
                        // response expects physical pixel dimensions.
                        cell_width: protocol_pixels(cell_width, scale_factor),
                        cell_height: protocol_pixels(cell_height, scale_factor),
                    };
                    pty_replies.extend_from_slice(format(size).as_bytes());
                }
                TerminalEvent::Exit => {
                    if let Some(observer) = &self.exit_observer {
                        observer();
                    }
                    self.queue_ui_event(TerminalEvent::Exit);
                }
            }
        }

        // Protocol replies share the PTY writer with user input, but do not
        // change scroll position: they are responses to the parser, not typing.
        if !pty_replies.is_empty() {
            let _ = self.write_pty_bytes(&pty_replies);
        }
    }

    fn queue_ui_event(&mut self, event: TerminalEvent) {
        // Window-dependent callbacks wait for the next visible render. Keep the
        // backlog bounded, coalesce title changes, and retain process exit.
        if matches!(&event, TerminalEvent::Title(_))
            && let Some(index) = self
                .pending_ui_events
                .iter()
                .rposition(|pending| matches!(pending, TerminalEvent::Title(_)))
        {
            self.pending_ui_events.remove(index);
        }

        if matches!(&event, TerminalEvent::Exit)
            && self
                .pending_ui_events
                .iter()
                .any(|pending| matches!(pending, TerminalEvent::Exit))
        {
            return;
        }

        if self.pending_ui_events.len() >= PENDING_UI_EVENT_LIMIT {
            let removable = self
                .pending_ui_events
                .iter()
                .position(|pending| !matches!(pending, TerminalEvent::Exit));
            if let Some(index) = removable {
                self.pending_ui_events.remove(index);
            } else {
                return;
            }
        }

        self.pending_ui_events.push_back(event);
    }

    fn process_pending_callbacks(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        while let Some(event) = self.pending_ui_events.pop_front() {
            match event {
                TerminalEvent::Bell => {
                    if let Some(ref callback) = self.bell_callback {
                        callback(window, cx);
                    }
                }
                TerminalEvent::Title(title) => {
                    if let Some(ref callback) = self.title_callback {
                        callback(window, cx, &title);
                    }
                }
                TerminalEvent::ClipboardStore(text) => {
                    if let Some(ref callback) = self.clipboard_store_callback {
                        callback(window, cx, &text);
                    }
                }
                TerminalEvent::Exit => {
                    if let Some(ref callback) = self.exit_callback {
                        callback(window, cx);
                    }
                }
                TerminalEvent::Wakeup
                | TerminalEvent::ClipboardLoad(_)
                | TerminalEvent::PtyWrite(_)
                | TerminalEvent::ColorRequest(_, _)
                | TerminalEvent::TextAreaSizeRequest(_) => {}
            }
        }
    }

    pub fn dimensions(&self) -> (usize, usize) {
        (self.state.cols(), self.state.rows())
    }

    pub fn resize(&mut self, cols: usize, rows: usize) {
        self.state.resize(cols, rows);
    }

    pub fn config(&self) -> &TerminalConfig {
        &self.config
    }

    pub fn focus_handle(&self) -> &FocusHandle {
        &self.focus_handle
    }

    pub fn update_config(&mut self, config: TerminalConfig, cx: &mut Context<Self>) {
        self.renderer.font_family = config.font_family.clone();
        self.renderer.font_size = config.font_size;
        self.renderer.line_height_multiplier = config.line_height_multiplier;
        self.renderer.minimum_contrast = config.minimum_contrast;
        self.renderer.palette = config.colors.clone();

        self.config = config;

        // The next canvas paint remeasures the configured font and resizes both
        // the terminal grid and the host PTY through the existing callback.
        cx.notify();
    }

    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.state.clear_screen();
        self.ime_text.clear();
        self.ime_marked_range = None;
        cx.notify();
    }

    pub fn has_selection(&self) -> bool {
        self.state
            .selection_text()
            .is_some_and(|text| !text.is_empty())
    }

    pub fn copy_selection(&self) -> bool {
        if let Some(text) = self.state.selection_text()
            && !text.is_empty()
            && let Ok(mut clipboard) = arboard::Clipboard::new()
        {
            let _ = clipboard.set_text(text);
            return true;
        }
        false
    }

    pub fn paste_from_clipboard(&self) -> bool {
        if let Ok(mut clipboard) = arboard::Clipboard::new()
            && let Ok(text) = clipboard.get_text()
        {
            return self.paste_text(&text);
        }
        false
    }

    pub fn paste_text(&self, text: &str) -> bool {
        let text = normalize_paste_text(text);
        if text.is_empty() {
            return false;
        }

        let mut bytes = Vec::with_capacity(text.len() + 12);
        if self.state.mode().contains(TermMode::BRACKETED_PASTE) {
            bytes.extend_from_slice(b"\x1b[200~");
        }
        bytes.extend_from_slice(text.as_bytes());
        if self.state.mode().contains(TermMode::BRACKETED_PASTE) {
            bytes.extend_from_slice(b"\x1b[201~");
        }

        self.write_input_bytes(&bytes)
    }

    fn write_pty_bytes(&self, bytes: &[u8]) -> bool {
        let mut writer = self.stdin_writer.lock();
        if writer.write_all(bytes).is_err() {
            return false;
        }
        if writer.flush().is_err() {
            return false;
        }
        true
    }

    fn write_input_bytes(&self, bytes: &[u8]) -> bool {
        if !self.write_pty_bytes(bytes) {
            return false;
        }
        self.state.scroll_to_bottom();
        true
    }

    fn report_focus(&self, focused: bool) {
        if !self.state.mode().contains(TermMode::FOCUS_IN_OUT) {
            return;
        }

        let sequence = if focused { b"\x1b[I" } else { b"\x1b[O" };
        let _ = self.write_pty_bytes(sequence);
    }

    #[allow(dead_code)]
    fn calculate_dimensions(&self, bounds: Bounds<Pixels>) -> (usize, usize) {
        let width_f32: f32 = bounds.size.width.into();
        let height_f32: f32 = bounds.size.height.into();
        let cell_width_f32: f32 = self.renderer.cell_width.into();
        let cell_height_f32: f32 = self.renderer.cell_height.into();

        let cols = ((width_f32 / cell_width_f32) as usize).max(1);
        let rows = ((height_f32 / cell_height_f32) as usize).max(1);
        (cols, rows)
    }

    fn build_context_menu(
        view: WeakEntity<Self>,
        has_selection: bool,
        menu: PopupMenu,
        _window: &mut Window,
        _cx: &mut Context<PopupMenu>,
    ) -> PopupMenu {
        menu.item(
            PopupMenuItem::new("Copy")
                .disabled(!has_selection)
                .on_click({
                    let view = view.clone();
                    move |_, _, cx| {
                        let _ = view.update(cx, |this, _| {
                            let _ = this.copy_selection();
                        });
                    }
                }),
        )
        .item(PopupMenuItem::new("Paste").on_click({
            let view = view.clone();
            move |_, _, cx| {
                let _ = view.update(cx, |this, cx| {
                    let _ = this.paste_from_clipboard();
                    cx.notify();
                });
            }
        }))
        .item(PopupMenuItem::separator())
        .item(PopupMenuItem::new("Clear").on_click({
            move |_, _, cx| {
                let _ = view.update(cx, |this, cx| {
                    this.clear(cx);
                });
            }
        }))
    }
}

impl EntityInputHandler for TerminalView {
    fn text_for_range(
        &mut self,
        _range_utf16: std::ops::Range<usize>,
        adjusted_range: &mut Option<std::ops::Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let range = self
            .ime_marked_range
            .clone()
            .unwrap_or(0..self.ime_text.encode_utf16().count());
        *adjusted_range = Some(range);
        Some(self.ime_text.clone())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: 0..0,
            reversed: false,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<std::ops::Range<usize>> {
        self.ime_marked_range.clone()
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.ime_marked_range = None;
        self.ime_text.clear();
        _cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        _range: Option<std::ops::Range<usize>>,
        text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.ime_marked_range = None;
        self.ime_text.clear();
        if !text.is_empty() {
            let _ = self.write_input_bytes(text.as_bytes());
        }
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _range: Option<std::ops::Range<usize>>,
        new_text: &str,
        new_selected_range: Option<std::ops::Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.ime_text = new_text.to_string();
        self.ime_marked_range = if new_text.is_empty() {
            None
        } else {
            new_selected_range.or_else(|| Some(0..new_text.encode_utf16().count()))
        };
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        _range_utf16: std::ops::Range<usize>,
        _element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let metrics = *self.layout_metrics.lock();
        self.state.with_term(|term| {
            let cursor = term.grid().cursor.point;
            let offset = term.grid().display_offset() as i32;
            let visible_row = cursor.line.0 + offset;
            Some(Bounds {
                origin: Point {
                    x: metrics.origin.x + metrics.cell_width * (cursor.column.0 as f32),
                    y: metrics.origin.y + metrics.cell_height * (visible_row as f32),
                },
                size: Size {
                    width: metrics.cell_width.max(px(1.0)),
                    height: metrics.cell_height.max(px(1.0)),
                },
            })
        })
    }

    fn character_index_for_point(
        &mut self,
        _point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        Some(0)
    }
}

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.focus_subscriptions.is_none() {
            let handle = self.focus_handle.clone();
            let focus = cx.on_focus(&handle, window, |view, _, _| view.report_focus(true));
            let blur = cx.on_blur(&handle, window, |view, _, _| view.report_focus(false));
            self.focus_subscriptions = Some((focus, blur));
        }

        self.process_output_events();
        self.process_pending_callbacks(window, cx);

        let state_arc = self.state.term_arc();
        let state_dimensions = self.state.dimensions_handle();
        let renderer = self.renderer.clone();
        let resize_callback = self.resize_callback.clone();
        let padding = self.config.padding;
        let layout_metrics = self.layout_metrics.clone();
        let focus_handle = self.focus_handle.clone();
        let entity = cx.entity();
        let ime_text = self.ime_text.clone();
        let ime_marked_range = self.ime_marked_range.clone();

        let terminal = div()
            .size_full()
            .bg(self.config.colors.background())
            .track_focus(&self.focus_handle)
            .key_context(CONTEXT)
            .on_action(cx.listener(Self::on_action_send_tab))
            .on_action(cx.listener(Self::on_action_send_backtab))
            .on_key_down(cx.listener(Self::on_key_down))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_down(MouseButton::Middle, cx.listener(Self::on_mouse_down))
            .on_mouse_down(MouseButton::Right, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up(MouseButton::Middle, cx.listener(Self::on_mouse_up))
            .on_mouse_up(MouseButton::Right, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_scroll(cx.listener(Self::on_scroll))
            .child(
                canvas(
                    move |bounds, _window, _cx| bounds,
                    move |bounds, _, window, cx| {
                        use alacritty_terminal::grid::Dimensions;

                        window.handle_input(
                            &focus_handle,
                            ElementInputHandler::new(bounds, entity.clone()),
                            cx,
                        );

                        let mut measured_renderer = renderer.clone();
                        measured_renderer.measure_cell(window);

                        let available_width: f32 =
                            (bounds.size.width - padding.left - padding.right).into();
                        let available_height: f32 =
                            (bounds.size.height - padding.top - padding.bottom).into();
                        let cell_width_f32: f32 = measured_renderer.cell_width.into();
                        let cell_height_f32: f32 = measured_renderer.cell_height.into();

                        let cols = ((available_width / cell_width_f32) as usize).max(1);
                        let rows = ((available_height / cell_height_f32) as usize).max(1);

                        struct TermSize {
                            cols: usize,
                            rows: usize,
                        }
                        impl Dimensions for TermSize {
                            fn total_lines(&self) -> usize {
                                self.rows
                            }
                            fn screen_lines(&self) -> usize {
                                self.rows
                            }
                            fn columns(&self) -> usize {
                                self.cols
                            }
                            fn last_column(&self) -> alacritty_terminal::index::Column {
                                alacritty_terminal::index::Column(self.cols.saturating_sub(1))
                            }
                            fn bottommost_line(&self) -> alacritty_terminal::index::Line {
                                alacritty_terminal::index::Line(self.rows as i32 - 1)
                            }
                            fn topmost_line(&self) -> alacritty_terminal::index::Line {
                                alacritty_terminal::index::Line(0)
                            }
                        }

                        let mut term = state_arc.lock();
                        let current_cols = term.columns();
                        let current_rows = term.screen_lines();
                        if cols != current_cols || rows != current_rows {
                            if let Some(ref callback) = resize_callback {
                                callback(cols, rows);
                            }
                            term.resize(TermSize { cols, rows });
                            *state_dimensions.lock() = (cols, rows);
                        }

                        let origin = Point {
                            x: bounds.origin.x + padding.left,
                            y: bounds.origin.y + padding.top,
                        };
                        *layout_metrics.lock() = LayoutMetrics {
                            origin,
                            cell_width: measured_renderer.cell_width,
                            cell_height: measured_renderer.cell_height,
                            cols,
                            rows,
                            scale_factor: window.scale_factor(),
                        };

                        measured_renderer.paint(
                            bounds,
                            padding,
                            &term,
                            window,
                            cx,
                            (!ime_text.is_empty())
                                .then_some((ime_text.as_str(), ime_marked_range.as_ref())),
                        );
                    },
                )
                .size_full(),
            );
        if self.mouse_reporting() {
            terminal.into_any_element()
        } else {
            let view = cx.entity().downgrade();
            let has_selection = self.has_selection();
            terminal
                .context_menu(move |menu, window, cx| {
                    Self::build_context_menu(view.clone(), has_selection, menu, window, cx)
                })
                .into_any_element()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::TerminalConfig;

    #[::core::prelude::v1::test]
    fn test_terminal_config_default_uses_compact_line_height() {
        let config = TerminalConfig::default();

        assert_eq!(config.line_height_multiplier, 1.0);
    }
}

// Protocol button numbers are distinct from GPUI's platform button enum.
fn mouse_button_code(button: MouseButton) -> Option<u8> {
    match button {
        MouseButton::Left => Some(0),
        MouseButton::Middle => Some(1),
        MouseButton::Right => Some(2),
        _ => None,
    }
}
