//! Native PDF viewing with one document worker per pane and a bounded bitmap cache.
//! PDF bytes are owned by the worker, so rebuilding the source PDF never holds a disk lock.
use gpui_kit::Pixels;
use gpui_kit::{
    component::input::{Input, InputEvent, InputState},
    prelude::FluentBuilder,
    *,
};
use pdfium_render::prelude::*;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};

struct PageInfo {
    ratio: f32,
}
struct PageImage {
    image: Arc<RenderImage>,
    text: String,
    characters: Vec<(String, [f32; 4])>,
}
struct Bookmark {
    title: String,
    page: usize,
    depth: usize,
}
enum PdfEvent {
    Opened(Vec<PageInfo>, Vec<Bookmark>),
    Rendered(usize, i32, PageImage),
    Thumbnail(usize, Arc<RenderImage>),
    Error(String),
}
struct RenderPage {
    index: usize,
    width: i32,
    thumbnail: bool,
    generation: u64,
}

pub struct PdfPane {
    path: PathBuf,
    pages: Vec<PageInfo>,
    images: BTreeMap<usize, PageImage>,
    text_pages: BTreeMap<usize, Vec<String>>,
    pending: BTreeSet<usize>,
    requests: mpsc::Sender<RenderPage>,
    events: mpsc::Receiver<PdfEvent>,
    list: ListState,
    width: f32,
    viewport_height: f32,
    zoom: f32,
    error: Option<String>,
    bookmarks: Vec<Bookmark>,
    navigation: u8,
    selection: Option<((usize, usize), (usize, usize))>,
    page_bounds: BTreeMap<usize, Bounds<Pixels>>,
    focus: FocusHandle,
    thumbnails: BTreeMap<usize, Arc<RenderImage>>,
    thumbnail_pending: BTreeSet<usize>,
    thumbnail_list: ListState,
    page_input: Option<Entity<InputState>>,
    generation: Arc<AtomicU64>,
    visible: bool,
    _tick: Task<()>,
}

impl PdfPane {
    pub fn new(path: PathBuf, cx: &mut Context<Self>) -> Self {
        let (requests, receiver) = mpsc::channel::<RenderPage>();
        let (sender, events) = mpsc::channel();
        let worker_path = path.clone();
        let generation = Arc::new(AtomicU64::new(0));
        let worker_generation = generation.clone();
        thread::spawn(move || {
            let run = || -> Result<(), Box<dyn std::error::Error>> {
                let bundled = std::env::current_exe()?.with_file_name("pdfium.dll");
                let library = if bundled.exists() {
                    bundled
                } else {
                    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("runtime/bin/pdfium.dll")
                };
                // pdfium-render keeps process-wide bindings; subsequent panes reuse them.
                let pdfium = match Pdfium::bind_to_library(library) {
                    Ok(bindings) => Pdfium::new(bindings),
                    Err(PdfiumError::PdfiumLibraryBindingsAlreadyInitialized) => Pdfium::default(),
                    Err(error) => return Err(error.into()),
                };
                let document = pdfium.load_pdf_from_byte_vec(fs::read(worker_path)?, None)?;
                let pages = document
                    .pages()
                    .iter()
                    .map(|page| PageInfo {
                        ratio: page.height().value / page.width().value,
                    })
                    .collect();
                let bookmarks = document
                    .bookmarks()
                    .iter()
                    .filter_map(|bookmark| {
                        let title = bookmark.title()?;
                        let destination = bookmark.destination()?;
                        let page = match destination.page_index() {
                            Ok(page) => page as usize,
                            Err(_) => return None,
                        };
                        let mut depth = 0;
                        let mut parent = bookmark.parent();
                        while let Some(item) = parent {
                            depth += 1;
                            parent = item.parent();
                        }
                        Some(Bookmark { title, page, depth })
                    })
                    .collect();
                if sender.send(PdfEvent::Opened(pages, bookmarks)).is_err() {
                    return Ok(());
                }
                for request in receiver {
                    // Discard queued work invalidated by a resize, zoom or hidden tab.
                    if request.generation != worker_generation.load(Ordering::Relaxed) {
                        continue;
                    }
                    let page = document.pages().get(request.index as i32)?;
                    let bitmap = page.render_with_config(
                        &PdfRenderConfig::new().set_target_width(request.width),
                    )?;
                    let mut rgba = bitmap.as_image()?.into_rgba8();
                    // GPUI's RenderImage uploads BGRA rather than the image crate's RGBA.
                    for pixel in rgba.chunks_exact_mut(4) {
                        pixel.swap(0, 2);
                    }
                    let image = Arc::new(RenderImage::new(vec![image::Frame::new(rgba)]));
                    if request.thumbnail {
                        if sender
                            .send(PdfEvent::Thumbnail(request.index, image))
                            .is_err()
                        {
                            break;
                        }
                        continue;
                    }
                    let text_page = page.text()?;
                    let text = text_page.all();
                    let mut characters = Vec::new();
                    for character in text_page.chars().iter() {
                        let bounds = character.tight_bounds()?;
                        characters.push((
                            character.unicode_string().unwrap_or_default(),
                            [
                                bounds.left().value / page.width().value,
                                1. - bounds.top().value / page.height().value,
                                bounds.width().value / page.width().value,
                                bounds.height().value / page.height().value,
                            ],
                        ));
                    }
                    if sender
                        .send(PdfEvent::Rendered(
                            request.index,
                            request.width,
                            PageImage {
                                image,
                                text,
                                characters,
                            },
                        ))
                        .is_err()
                    {
                        break;
                    }
                }
                Ok(())
            };
            if let Err(error) = run() {
                let _ = sender.send(PdfEvent::Error(error.to_string()));
            }
        });
        let tick = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(50))
                    .await;
                if this
                    .update(cx, |pane, cx| {
                        let events: Vec<_> = pane.events.try_iter().collect();
                        if events.is_empty() {
                            return;
                        }
                        for event in events {
                            match event {
                                PdfEvent::Opened(pages, bookmarks) => {
                                    pane.pages = pages;
                                    pane.bookmarks = bookmarks;
                                    pane.list.reset(pane.pages.len());
                                    pane.thumbnail_list.reset(pane.pages.len());
                                }
                                PdfEvent::Rendered(index, width, image) => {
                                    pane.pending.remove(&index);
                                    if pane.visible && width == pane.pixel_width() {
                                        pane.text_pages.insert(
                                            index,
                                            image
                                                .characters
                                                .iter()
                                                .map(|(text, _)| text.clone())
                                                .collect(),
                                        );
                                        pane.images.insert(index, image);
                                    }
                                    // Keep nearby pages only; closed panes drop their worker and bitmaps.
                                    let top = pane.list.logical_scroll_top().item_ix;
                                    let ratio = pane
                                        .pages
                                        .iter()
                                        .map(|page| page.ratio)
                                        .fold(f32::INFINITY, f32::min);
                                    let count = (pane.viewport_height
                                        / (pane.width * pane.zoom * ratio + 24.))
                                        .ceil()
                                        as usize
                                        + 1;
                                    pane.images.retain(|index, _| {
                                        *index >= top.saturating_sub(1) && *index <= top + count
                                    });
                                    pane.text_pages.retain(|index, _| {
                                        pane.images.contains_key(index)
                                            || pane.selection.is_some_and(|(a, b)| {
                                                *index >= a.0.min(b.0) && *index <= a.0.max(b.0)
                                            })
                                    });
                                }
                                PdfEvent::Thumbnail(index, image) => {
                                    pane.thumbnail_pending.remove(&index);
                                    if pane.visible {
                                        pane.thumbnails.insert(index, image);
                                    }
                                    let top = pane.thumbnail_list.logical_scroll_top().item_ix;
                                    pane.thumbnails.retain(|index, _| index.abs_diff(top) <= 10);
                                }
                                PdfEvent::Error(error) => pane.error = Some(error),
                            }
                        }
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        Self {
            path,
            pages: Vec::new(),
            images: BTreeMap::new(),
            text_pages: BTreeMap::new(),
            pending: BTreeSet::new(),
            requests,
            events,
            list: ListState::new(0, ListAlignment::Top, px(100.)),
            width: 500.,
            viewport_height: 900.,
            zoom: 1.,
            error: None,
            bookmarks: Vec::new(),
            navigation: 0,
            selection: None,
            page_bounds: BTreeMap::new(),
            focus: cx.focus_handle(),
            thumbnails: BTreeMap::new(),
            thumbnail_pending: BTreeSet::new(),
            thumbnail_list: ListState::new(0, ListAlignment::Top, px(150.)),
            page_input: None,
            generation,
            visible: true,
            _tick: tick,
        }
    }
    fn pixel_width(&self) -> i32 {
        (self.width * self.zoom * 1.5).round().min(4096.) as i32
    }
    pub fn set_visible(&mut self, visible: bool) {
        if self.visible == visible {
            return;
        }
        self.visible = visible;
        if !visible {
            self.generation.fetch_add(1, Ordering::Relaxed);
            self.images.clear();
            self.pending.clear();
            self.thumbnails.clear();
            self.thumbnail_pending.clear();
            self.page_bounds.clear();
            self.text_pages.clear();
            self.selection = None;
        }
    }
    fn resize(&mut self, width: f32, height: f32, cx: &mut Context<Self>) {
        self.viewport_height = height;
        if (self.width - width).abs() < 2. {
            return;
        }
        self.width = width.max(80.);
        self.generation.fetch_add(1, Ordering::Relaxed);
        self.images.clear();
        self.pending.clear();
        self.thumbnail_pending.clear();
        self.list.reset(self.pages.len());
        cx.notify();
    }
    fn zoom(&mut self, amount: f32, cx: &mut Context<Self>) {
        self.zoom = (self.zoom + amount).clamp(0.25, 3.);
        self.generation.fetch_add(1, Ordering::Relaxed);
        self.images.clear();
        self.pending.clear();
        self.thumbnail_pending.clear();
        self.list.reset(self.pages.len());
        cx.notify();
    }
    fn page(&mut self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let width = self.width * self.zoom;
        let height = width * self.pages[index].ratio;
        if !self.images.contains_key(&index) && self.pending.insert(index) {
            if let Err(error) = self.requests.send(RenderPage {
                index,
                width: self.pixel_width(),
                thumbnail: false,
                generation: self.generation.load(Ordering::Relaxed),
            }) {
                self.error = Some(error.to_string());
            }
        }
        let mut page = div()
            .relative()
            .w(px(width))
            .h(px(height))
            .bg(rgb(0xffffff))
            .shadow_sm();
        if let Some(image) = self.images.get(&index) {
            page = page.child(img(image.image.clone()).size_full());
            if let Some((start, end)) = self.selection {
                let (start, end) = (start.min(end), start.max(end));
                for (offset, (_, rect)) in image.characters.iter().enumerate() {
                    if (index, offset) >= start && (index, offset) <= end {
                        page = page.child(
                            div()
                                .absolute()
                                .left(px(rect[0] * width))
                                .top(px(rect[1] * height))
                                .w(px(rect[2] * width))
                                .h(px(rect[3] * height))
                                .bg(rgba(0x82aee35a)),
                        );
                    }
                }
            }
            let measured = cx.entity();
            page = page.child(
                canvas(
                    move |bounds, _, cx| {
                        measured.update(cx, |pane, _| {
                            pane.page_bounds.insert(index, bounds);
                        })
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            );
            let text = image.text.clone();
            return div()
                .flex()
                .flex_col()
                .items_center()
                .py_3()
                .child(
                    page.id(("page", index))
                        .cursor_text()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |pane, event: &MouseDownEvent, window, cx| {
                                pane.focus.focus(window, cx);
                                if let Some(character) = pane.hit_character(index, event.position) {
                                    pane.selection = Some(((index, character), (index, character)));
                                    cx.notify();
                                }
                            }),
                        )
                        .on_mouse_move(cx.listener(move |pane, event: &MouseMoveEvent, _, cx| {
                            if event.pressed_button != Some(MouseButton::Left) {
                                return;
                            }
                            if let Some(character) = pane.hit_character(index, event.position) {
                                if let Some((_, end)) = &mut pane.selection {
                                    *end = (index, character);
                                    cx.notify();
                                }
                            }
                        }))
                        .context_menu(move |menu, _, _| {
                            let text = text.clone();
                            menu.item(
                                gpui_kit::component::menu::PopupMenuItem::new("复制此页文字")
                                    .on_click(move |_, _, cx| {
                                        cx.write_to_clipboard(ClipboardItem::new_string(
                                            text.clone(),
                                        ))
                                    }),
                            )
                        }),
                )
                .into_any_element();
        }
        let _ = cx;
        div()
            .flex()
            .flex_col()
            .items_center()
            .py_3()
            .child(
                page.child(
                    div()
                        .p_4()
                        .text_color(rgb(0x9b9a97))
                        .child(format!("第 {} 页 · 渲染中…", index + 1)),
                ),
            )
            .into_any_element()
    }

    fn hit_character(&self, page: usize, position: Point<Pixels>) -> Option<usize> {
        let bounds = self.page_bounds.get(&page)?;
        let image = self.images.get(&page)?;
        let x = f32::from(position.x - bounds.origin.x) / f32::from(bounds.size.width);
        let y = f32::from(position.y - bounds.origin.y) / f32::from(bounds.size.height);
        image
            .characters
            .iter()
            .enumerate()
            .min_by(|(_, (_, a)), (_, (_, b))| {
                let distance = |rect: &[f32; 4]| {
                    (x - (rect[0] + rect[2] / 2.)).powi(2)
                        + 9. * (y - (rect[1] + rect[3] / 2.)).powi(2)
                };
                distance(a).total_cmp(&distance(b))
            })
            .map(|(index, _)| index)
    }

    fn copy_selection(&self, cx: &mut App) {
        if let Some((a, b)) = self.selection {
            let start = a.min(b);
            let end = a.max(b);
            let mut text = String::new();
            for (page, characters) in &self.text_pages {
                for (index, character) in characters.iter().enumerate() {
                    if (*page, index) >= start && (*page, index) <= end {
                        text.push_str(character);
                    }
                }
            }
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    fn thumbnail(&mut self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        if !self.thumbnails.contains_key(&index) && self.thumbnail_pending.insert(index) {
            if let Err(error) = self.requests.send(RenderPage {
                index,
                width: 160,
                thumbnail: true,
                generation: self.generation.load(Ordering::Relaxed),
            }) {
                self.error = Some(error.to_string());
            }
        }
        div()
            .id(("thumbnail", index))
            .h(px(164.))
            .p_2()
            .cursor_pointer()
            .child(
                div()
                    .h(px(140.))
                    .w_full()
                    .bg(rgb(0xffffff))
                    .border_1()
                    .border_color(rgb(if index == self.list.logical_scroll_top().item_ix {
                        0x7195bc
                    } else {
                        0xe8e7e3
                    }))
                    .when_some(self.thumbnails.get(&index), |row, image| {
                        row.child(
                            img(image.clone())
                                .size_full()
                                .object_fit(ObjectFit::Contain),
                        )
                    }),
            )
            .child(
                div()
                    .text_xs()
                    .text_center()
                    .child(format!("{}", index + 1)),
            )
            .on_click(cx.listener(move |pane, _, _, cx| {
                pane.list.scroll_to_reveal_item(index);
                cx.notify();
            }))
            .into_any_element()
    }

    fn navigation(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut sidebar = div()
            .id("pdf-navigation")
            .w(px(if self.navigation == 1 { 200. } else { 120. }))
            .flex_shrink_0()
            .h_full()
            .overflow_y_scroll()
            .p_2()
            .border_r_1()
            .border_color(rgb(0xe8e7e3));
        if self.navigation == 1 {
            if self.bookmarks.is_empty() {
                sidebar = sidebar.child("此文档没有目录");
            }
            for (index, bookmark) in self.bookmarks.iter().enumerate() {
                let page = bookmark.page;
                sidebar = sidebar.child(
                    div()
                        .id(("bookmark", index))
                        .pl(px(6. + bookmark.depth as f32 * 12.))
                        .py_2()
                        .text_xs()
                        .cursor_pointer()
                        .hover(|style| style.bg(rgb(0xe8e7e4)))
                        .child(bookmark.title.clone())
                        .on_click(cx.listener(move |pane, _, _, cx| {
                            pane.list.scroll_to_reveal_item(page);
                            cx.notify();
                        })),
                );
            }
        } else {
            let entity = cx.entity();
            return div()
                .w(px(130.))
                .h_full()
                .flex_shrink_0()
                .child(
                    list(self.thumbnail_list.clone(), move |index, _, cx| {
                        entity.update(cx, |pane, cx| pane.thumbnail(index, cx))
                    })
                    .size_full(),
                )
                .into_any_element();
        }
        sidebar.into_any_element()
    }
}

use gpui_kit::component::menu::ContextMenuExt;

impl Render for PdfPane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.page_input.is_none() {
            let input = cx.new(|cx| InputState::new(window, cx).default_value("1"));
            cx.subscribe(&input, |pane, input, event: &InputEvent, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    if let Ok(page) = input.read(cx).value().trim().parse::<usize>() {
                        if page > 0 && page <= pane.pages.len() {
                            pane.list.scroll_to_reveal_item(page - 1);
                            cx.notify();
                        }
                    }
                }
            })
            .detach();
            self.page_input = Some(input);
        }
        let entity = cx.entity();
        let measured = entity.clone();
        let top = self.list.logical_scroll_top().item_ix;
        let page_input = self
            .page_input
            .as_ref()
            .expect("page input initialized")
            .clone();
        let current_page = (top + 1).to_string();
        if !page_input.read(cx).focus_handle(cx).is_focused(window)
            && page_input.read(cx).value().as_ref() != current_page
        {
            page_input.update(cx, |input, cx| input.set_value(current_page, window, cx));
        }
        let path = self.path.clone();
        let export = self.path.clone();
        div()
            .key_context("FolioPdf")
            .track_focus(&self.focus)
            .on_action(
                cx.listener(|pane, _: &gpui_kit::component::input::Copy, _, cx| {
                    pane.copy_selection(cx)
                }),
            )
            .on_scroll(cx.listener(|pane, event: &ScrollEvent, _, cx| {
                if event.modifiers.control {
                    pane.zoom(
                        if event.delta.pixel_delta(px(20.)).y > px(0.) {
                            0.1
                        } else {
                            -0.1
                        },
                        cx,
                    );
                    cx.stop_propagation();
                }
            }))
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(0xf1f1ef))
            .child(
                div()
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .justify_between()
                    .h(px(36.))
                    .px_3()
                    .bg(rgb(0xffffff))
                    .border_b_1()
                    .border_color(rgb(0xe8e7e3))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .child("预览")
                            .child(
                                Input::new(
                                    self.page_input.as_ref().expect("page input initialized"),
                                )
                                .w(px(48.)),
                            )
                            .child(format!("/ {}", self.pages.len())),
                    )
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(
                                div()
                                    .id("pdf-outline")
                                    .cursor_pointer()
                                    .child(crate::ui::label("☷".into()))
                                    .on_click(cx.listener(|pane, _, _, cx| {
                                        pane.navigation = if pane.navigation == 1 { 0 } else { 1 };
                                        cx.notify();
                                    })),
                            )
                            .child(
                                div()
                                    .id("pdf-pages")
                                    .cursor_pointer()
                                    .child(crate::ui::label("▦".into()))
                                    .on_click(cx.listener(|pane, _, _, cx| {
                                        pane.navigation = if pane.navigation == 2 { 0 } else { 2 };
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .gap_3()
                            .child(
                                div()
                                    .id("pdf-previous")
                                    .cursor_pointer()
                                    .child("‹")
                                    .on_click(cx.listener(move |pane, _, _, cx| {
                                        pane.list.scroll_to_reveal_item(top.saturating_sub(1));
                                        cx.notify();
                                    })),
                            )
                            .child(div().id("pdf-next").cursor_pointer().child("›").on_click(
                                cx.listener(move |pane, _, _, cx| {
                                    if top + 1 < pane.pages.len() {
                                        pane.list.scroll_to_reveal_item(top + 1);
                                        cx.notify();
                                    }
                                }),
                            ))
                            .child(
                                div()
                                    .id("pdf-minus")
                                    .cursor_pointer()
                                    .child("−")
                                    .on_click(cx.listener(|pane, _, _, cx| pane.zoom(-0.1, cx))),
                            )
                            .child(
                                div()
                                    .id("pdf-fit")
                                    .cursor_pointer()
                                    .child(format!("{}%", (self.zoom * 100.).round()))
                                    .on_click(cx.listener(|pane, _, _, cx| {
                                        pane.zoom = 1.;
                                        pane.zoom(0., cx);
                                    })),
                            )
                            .child(
                                div()
                                    .id("pdf-plus")
                                    .cursor_pointer()
                                    .child("+")
                                    .on_click(cx.listener(|pane, _, _, cx| pane.zoom(0.1, cx))),
                            )
                            .child(
                                div()
                                    .id("pdf-reveal")
                                    .cursor_pointer()
                                    .child("▱")
                                    .on_click(move |_, _, cx| cx.reveal_path(&path)),
                            )
                            .child(div().id("pdf-export").cursor_pointer().child("↓").on_click(
                                cx.listener(move |_, _, window, cx| {
                                    let picker = rfd::AsyncFileDialog::new()
                                        .set_parent(window)
                                        .set_title("导出 PDF")
                                        .set_file_name(
                                            export
                                                .file_name()
                                                .expect("PDF has filename")
                                                .to_string_lossy(),
                                        )
                                        .add_filter("PDF", &["pdf"]);
                                    let failure = rfd::AsyncMessageDialog::new()
                                        .set_parent(window)
                                        .set_title("PDF 导出失败");
                                    let source = export.clone();
                                    cx.spawn_in(window, async move |_, _| {
                                        if let Some(destination) = picker.save_file().await {
                                            if let Err(error) =
                                                fs::copy(&source, destination.path())
                                            {
                                                failure
                                                    .set_description(error.to_string())
                                                    .show()
                                                    .await;
                                            }
                                        }
                                    })
                                    .detach();
                                }),
                            )),
                    ),
            )
            .child(
                div()
                    .flex()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .overflow_hidden()
                    .when(self.navigation != 0, |area| area.child(self.navigation(cx)))
                    .child(
                        div()
                            .id("pdf-horizontal")
                            .relative()
                            .flex_1()
                            .min_w_0()
                            .min_h_0()
                            .overflow_x_scroll()
                            .child(
                                canvas(
                                    move |bounds, _, cx| {
                                        measured.update(cx, |pane, cx| {
                                            pane.resize(
                                                f32::from(bounds.size.width) - 32.,
                                                f32::from(bounds.size.height),
                                                cx,
                                            )
                                        })
                                    },
                                    |_, _, _, _| {},
                                )
                                .absolute()
                                .h_full()
                                .w(px((self.width * self.zoom + 32.).max(self.width + 32.))),
                            )
                            .when_some(self.error.clone(), |area, error| {
                                area.child(div().p_4().text_color(rgb(0xa16b60)).child(error))
                            })
                            .child(
                                list(self.list.clone(), move |index, _, cx| {
                                    entity.update(cx, |pane, cx| pane.page(index, cx))
                                })
                                .size_full(),
                            ),
                    ),
            )
    }
}
