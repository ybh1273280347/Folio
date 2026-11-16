//! Shared Lucide controls use the same restrained icon treatment as Folio's web UI.
use crate::theme::color as rgb;
use gpui_kit::{
    assets::{IconName, icon_assets},
    base::StyledExt,
    component::{Icon, button::{Button, ButtonVariants}},
    prelude::FluentBuilder,
    *,
};
use std::borrow::Cow;

/// Read the current draft only when clicked; feedback belongs to this document's button.
pub fn copy_document_button(
    id: impl Into<ElementId>,
    read_text: impl Fn(&App) -> String + 'static,
) -> impl IntoElement {
    CopyDocumentButton { id: id.into(), read_text: Box::new(read_text) }
}

#[derive(IntoElement)]
struct CopyDocumentButton {
    id: ElementId,
    read_text: Box<dyn Fn(&App) -> String>,
}

struct CopyFeedback {
    copied: bool,
    reset: Task<()>,
}

impl RenderOnce for CopyDocumentButton {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let feedback = window.use_keyed_state(self.id.clone(), cx, |_, _| CopyFeedback {
            copied: false, reset: Task::ready(()),
        });
        let copied = feedback.read(cx).copied;
        Button::new(self.id).ghost().compact().w(px(28.)).h(px(28.))
            .icon(icon(if copied { IconName::Check } else { IconName::Copy }))
            .tooltip(if copied { "已复制全文" } else { "复制全文" })
            .on_click(move |_, _, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string((self.read_text)(cx)));
                feedback.update(cx, |feedback, cx| {
                    feedback.copied = true;
                    cx.notify();
                    // Repeated clicks replace the pending reset instead of starting a polling loop.
                    feedback.reset = cx.spawn(async move |this, cx| {
                        cx.background_executor().timer(std::time::Duration::from_millis(1500)).await;
                        let _ = this.update(cx, |feedback, cx| {
                            feedback.copied = false;
                            cx.notify();
                        });
                    });
                });
            })
    }
}

/// Keep preview, live Markdown and PDF typography relative to the same body size.
pub fn inline_code_style(settings: &crate::model::AppSettings) -> StyleRefinement {
    let mut style = StyleRefinement::default()
        .text_size(px(settings.markdown_font_size as f32 * settings.inline_code_scale));
    if settings.theme.palette != crate::theme::Palette::light() {
        style = style.bg(gpui_kit::rgb(settings.theme.palette.code_background));
    }
    if !settings.inline_code_font.is_empty() {
        style = style.font_family(settings.inline_code_font.clone());
    }
    style
}

pub fn block_code_style(settings: &crate::model::AppSettings) -> StyleRefinement {
    // Match the default 13px code at 14px body size, then scale with Ctrl+wheel.
    let mut style = StyleRefinement::default()
        .text_size(px(settings.markdown_font_size as f32 * 13. / 14.));
    if settings.theme.palette != crate::theme::Palette::light() {
        style = style.bg(gpui_kit::rgb(settings.theme.palette.code_background));
    }
    if !settings.block_code_font.is_empty() {
        style = style.font_family(settings.block_code_font.clone());
    }
    style
}

/// Insert before scrollable children so Ctrl+wheel is consumed in capture,
/// before their native bubble-phase scrolling can change the offset.
pub fn ctrl_scroll_capture(
    listener: impl Fn(&ScrollEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    canvas(
        |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
        move |_, hitbox, window, _| {
            window.on_mouse_event(move |event: &ScrollEvent, phase, window, cx| {
                if phase == DispatchPhase::Capture
                    && event.modifiers.control
                    && hitbox.should_handle_scroll(window)
                {
                    // Consume even at the zoom limits. Overlays occlude this
                    // hitbox, so their scrolling and inputs remain local.
                    cx.stop_propagation();
                    if event.delta.pixel_delta(px(20.)).y != px(0.) {
                        listener(event, window, cx);
                    }
                }
            });
        },
    )
    .absolute()
    .inset_0()
}

/// Transient bottom-right pill badge showing the current font size on Ctrl+wheel zoom.
pub struct ZoomHud {
    font_size: u8,
    visible: bool,
    fading: bool,
    generation: u64,
    fade_generation: u64,
    timer: Task<()>,
}

impl Default for ZoomHud {
    fn default() -> Self {
        Self {
            font_size: 14,
            visible: false,
            fading: false,
            generation: 0,
            fade_generation: 0,
            timer: Task::ready(()),
        }
    }
}

impl ZoomHud {
    pub fn show<T: 'static>(
        &mut self,
        font_size: u8,
        cx: &mut Context<T>,
        access: fn(&mut T) -> &mut Self,
    ) {
        if !self.visible {
            self.generation = self.generation.wrapping_add(1);
        }
        self.font_size = font_size;
        self.visible = true;
        self.fading = false;
        self.timer = cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(950))
                .await;
            let keep_going = this
                .update(cx, |owner, cx| {
                    let hud = access(owner);
                    if !hud.visible {
                        return false;
                    }
                    hud.fading = true;
                    hud.fade_generation = hud.fade_generation.wrapping_add(1);
                    cx.notify();
                    true
                })
                .unwrap_or(false);
            if !keep_going {
                return;
            }
            cx.background_executor()
                .timer(std::time::Duration::from_millis(250))
                .await;
            let _ = this.update(cx, |owner, cx| {
                let hud = access(owner);
                hud.visible = false;
                hud.fading = false;
                cx.notify();
            });
        });
    }

    pub fn render(&self, id: &'static str) -> Option<AnyElement> {
        if !self.visible {
            return None;
        }
        let mut bg = rgb(0xfafaf9);
        bg.a = 0.90;
        let mut border = rgb(0xe6e5e1);
        border.a = 0.90;

        let pill = div()
            .px_2p5()
            .py_1()
            .rounded_full()
            .bg(bg)
            .border_1()
            .border_color(border)
            .shadow(vec![
                BoxShadow::new(px(0.), px(2.), hsla(0., 0., 0., 0.08)).blur_radius(px(8.)),
            ])
            .text_xs()
            .font_medium()
            .text_color(rgb(0x5d5d5a))
            .child(format!("{}px", self.font_size));

        let animated = if self.fading {
            pill.with_animation(
                (id, self.fade_generation.wrapping_mul(2).wrapping_add(1)),
                Animation::new(std::time::Duration::from_millis(250))
                    .with_easing(ease_out_quint()),
                |el, t| el.opacity(1. - t),
            )
            .into_any_element()
        } else {
            pill.with_animation(
                (id, self.generation.wrapping_mul(2)),
                Animation::new(std::time::Duration::from_millis(120))
                    .with_easing(ease_out_quint()),
                |el, t| el.opacity(t),
            )
            .into_any_element()
        };

        Some(
            div()
                .absolute()
                .bottom_3()
                .right_3()
                .child(animated)
                .into_any_element(),
        )
    }
}

icon_assets!(
    FolioIcons,
    [
        ListTree,
        Images,
        Image,
        CodeXml,
        Code,
        PencilLine,
        NotebookPen,
        Files,
        Trash,
        SlidersHorizontal,
        FileCode,
        FileImage,
        Library,
        Columns2,
        Maximize2,
        PanelLeftClose,
        PanelLeftOpen,
        PanelRightOpen,
        Bold,
        Italic,
        Underline,
        Strikethrough,
        Highlighter,
        Heading1,
        Heading2,
        Heading3,
        Heading4,
        List,
        ListOrdered,
        ListTodo,
        TextQuote,
        Sigma,
        Table2,
        Ellipsis,
        Download,
        FolderPlus,
        Link,
        MessageSquarePlus,
        MessageSquareText,
        TextAlignStart,
        TextAlignCenter,
        TextAlignEnd,
        ListIndentIncrease,
        ListIndentDecrease,
        GripVertical,
        Keyboard,
        Palette,
        RotateCcw,
        Type,
        Sparkles,
        Copy,
        Check,
        Globe,
        ExternalLink,
        ChevronLeft,
        ChevronRight,
        ChevronUp,
        ChevronDown,
        Plus,
        Close,
        Eye,
        EyeOff,
        Terminal,
        SquareTerminal,
        Bot,
        Brain,
        Cpu,
        Wrench,
        FileText,
        FileDiff,
        SendHorizontal,
        Square,
        Clock,
        RefreshCw,
        Shield,
        ShieldCheck,
        ShieldAlert,
        GitBranch,
        Paperclip,
        ArrowUp,
        CircleDot
    ]
);

const BRAND_CLAUDE_SVG: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="48" height="48" viewBox="0 0 24 24" fill="#D97757"><path d="m4.7144 15.9555 4.7174-2.6471.079-.2307-.079-.1275h-.2307l-.7893-.0486-2.6956-.0729-2.3375-.0971-2.2646-.1214-.5707-.1215-.5343-.7042.0546-.3522.4797-.3218.686.0608 1.5179.1032 2.2767.1578 1.6514.0972 2.4468.255h.3886l.0546-.1579-.1336-.0971-.1032-.0972L6.973 9.8356l-2.55-1.6879-1.3356-.9714-.7225-.4918-.3643-.4614-.1578-1.0078.6557-.7225.8803.0607.2246.0607.8925.686 1.9064 1.4754 2.4893 1.8336.3643.3035.1457-.1032.0182-.0728-.164-.2733-1.3539-2.4467-1.445-2.4893-.6435-1.032-.17-.6194c-.0607-.255-.1032-.4674-.1032-.7285L6.287.1335 6.6997 0l.9957.1336.419.3642.6192 1.4147 1.0018 2.2282 1.5543 3.0296.4553.8985.2429.8318.091.255h.1579v-.1457l.1275-1.706.2368-2.0947.2307-2.6957.0789-.7589.3764-.9107.7468-.4918.5828.2793.4797.686-.0668.4433-.2853 1.8517-.5586 2.9021-.3643 1.9429h.2125l.2429-.2429.9835-1.3053 1.6514-2.0643.7286-.8196.85-.9046.5464-.4311h1.0321l.759 1.1293-.34 1.1657-1.0625 1.3478-.8804 1.1414-1.2628 1.7-.7893 1.36.0729.1093.1882-.0183 2.8535-.607 1.5421-.2794 1.8396-.3157.8318.3886.091.3946-.3278.8075-1.967.4857-2.3072.4614-3.4364.8136-.0425.0304.0486.0607 1.5482.1457.6618.0364h1.621l3.0175.2247.7892.522.4736.6376-.079.4857-1.2142.6193-1.6393-.3886-3.825-.9107-1.3113-.3279h-.1822v.1093l1.0929 1.0686 2.0035 1.8092 2.5075 2.3314.1275.5768-.3218.4554-.34-.0486-2.2039-1.6575-.85-.7468-1.9246-1.621h-.1275v.17l.4432.6496 2.3436 3.5214.1214 1.0807-.17.3521-.6071.2125-.6679-.1214-1.3721-1.9246L14.38 17.959l-1.1414-1.9428-.1397.079-.674 7.2552-.3156.3703-.7286.2793-.6071-.4614-.3218-.7468.3218-1.4753.3886-1.9246.3157-1.53.2853-1.9004.17-.6314-.0121-.0425-.1397.0182-1.4328 1.9672-2.1796 2.9446-1.7243 1.8456-.4128.164-.7164-.3704.0667-.6618.4008-.5889 2.386-3.0357 1.4389-1.882.929-1.0868-.0062-.1579h-.0546l-6.3385 4.1164-1.1293.1457-.4857-.4554.0608-.7467.2307-.2429 1.9064-1.3114Z"/></svg>"##;

const BRAND_CODEX_SVG: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="48" height="48" viewBox="0 0 256 260"><path fill="#10A37F" d="M239.184 106.203a64.716 64.716 0 0 0-5.576-53.103C219.452 28.459 191 15.784 163.213 21.74A65.586 65.586 0 0 0 52.096 45.22a64.716 64.716 0 0 0-43.23 31.36c-14.31 24.602-11.061 55.634 8.033 76.74a64.665 64.665 0 0 0 5.525 53.102c14.174 24.65 42.644 37.324 70.446 31.36a64.72 64.72 0 0 0 48.754 21.744c28.481.025 53.714-18.361 62.414-45.481a64.767 64.767 0 0 0 43.229-31.36c14.137-24.558 10.875-55.423-8.083-76.483Zm-97.56 136.338a48.397 48.397 0 0 1-31.105-11.255l1.535-.87 51.67-29.825a8.595 8.595 0 0 0 4.247-7.367v-72.85l21.845 12.636c.218.111.37.32.409.563v60.367c-.056 26.818-21.783 48.545-48.601 48.601Zm-104.466-44.61a48.345 48.345 0 0 1-5.781-32.589l1.534.921 51.722 29.826a8.339 8.339 0 0 0 8.441 0l63.181-36.425v25.221a.87.87 0 0 1-.358.665l-52.335 30.184c-23.257 13.398-52.97 5.431-66.404-17.803ZM23.549 85.38a48.499 48.499 0 0 1 25.58-21.333v61.39a8.288 8.288 0 0 0 4.195 7.316l62.874 36.272-21.845 12.636a.819.819 0 0 1-.767 0L41.353 151.53c-23.211-13.454-31.171-43.144-17.804-66.405v.256Zm179.466 41.695-63.08-36.63L161.73 77.86a.819.819 0 0 1 .768 0l52.233 30.184a48.6 48.6 0 0 1-7.316 87.635v-61.391a8.544 8.544 0 0 0-4.4-7.213Zm21.742-32.69-1.535-.922-51.619-30.081a8.39 8.39 0 0 0-8.492 0L99.98 99.808V74.587a.716.716 0 0 1 .307-.665l52.233-30.133a48.652 48.652 0 0 1 72.236 50.391v.205ZM88.061 139.097l-21.845-12.585a.87.87 0 0 1-.41-.614V65.685a48.652 48.652 0 0 1 79.757-37.346l-1.535.87-51.67 29.825a8.595 8.595 0 0 0-4.246 7.367l-.051 72.697Zm11.868-25.58 28.138-16.217 28.188 16.218v32.434l-28.086 16.218-28.188-16.218-.052-32.434Z"/></svg>"##;

const BRAND_PI_SVG: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="48" height="48" viewBox="0 0 800 800"><path fill="#F09082" d="M165.29 165.29H517.36V400H400V282.65H165.29Z"/><path fill="#4D9ABF" d="M165.29 282.65H282.65V400H400V517.36H282.65V634.72H165.29Z"/><path fill="#F1BE58" d="M517.36 400H634.72V634.72H517.36Z"/></svg>"##;

const BRAND_ANTIGRAVITY_SVG: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="48" height="48" viewBox="0 0 24 24"><defs><clipPath id="ag-clip"><path d="M21.751 22.607c1.34 1.005 3.35.335 1.508-1.508C17.73 15.74 18.904 1 12.037 1 5.17 1 6.342 15.74.815 21.1c-2.01 2.009.167 2.511 1.507 1.506 5.192-3.517 4.857-9.714 9.715-9.714 4.857 0 4.522 6.197 9.714 9.715z"/></clipPath></defs><g clip-path="url(#ag-clip)"><rect width="24" height="24" fill="#3186FF"/><path d="M-1.018-3.992c-.408 3.591 2.686 6.89 6.91 7.37 4.225.48 7.98-2.043 8.387-5.633.408-3.59-2.686-6.89-6.91-7.37-4.225-.479-7.98 2.043-8.387 5.633z" fill="#FFE432"/><path d="M15.269 7.747c1.058 4.557 5.691 7.374 10.348 6.293 4.657-1.082 7.575-5.653 6.516-10.21-1.058-4.556-5.691-7.374-10.348-6.292-4.657 1.082-7.575 5.653-6.516 10.21z" fill="#FC413D"/><path d="M-7.608 14.703c3.352 3.424 9.126 3.208 12.896-.483 3.77-3.69 4.108-9.459.756-12.883C2.69-2.087-3.083-1.871-6.853 1.82c-3.77 3.69-4.108 9.458-.755 12.883z" fill="#00B95C"/><path d="M9.932 27.617c1.04 4.482 5.384 7.303 9.7 6.3 4.316-1.002 6.971-5.448 5.93-9.93-1.04-4.483-5.384-7.304-9.7-6.301-4.316 1.002-6.971 5.448-5.93 9.93z" fill="#3186FF"/><path d="M18.163 9.077c5.81 3.93 12.502 4.19 14.946.577 2.443-3.612-.287-9.727-6.098-13.658-5.81-3.931-12.502-4.19-14.946-.577-2.443 3.612.287 9.727 6.098 13.658z" fill="#FC413D"/><path d="M-.915 2.684c-1.44 3.473-.97 6.967 1.05 7.804 2.02.837 4.824-1.3 6.264-4.772 1.44-3.473.97-6.967-1.05-7.804-2.02-.837-4.824 1.3-6.264 4.772z" fill="#FFEE48"/></g></svg>"##;

pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        match path {
            "brand/claude.svg" => return Ok(Some(Cow::Borrowed(BRAND_CLAUDE_SVG))),
            "brand/codex.svg" => return Ok(Some(Cow::Borrowed(BRAND_CODEX_SVG))),
            "brand/pi.svg" => return Ok(Some(Cow::Borrowed(BRAND_PI_SVG))),
            "brand/antigravity.svg" => return Ok(Some(Cow::Borrowed(BRAND_ANTIGRAVITY_SVG))),
            _ => {}
        }
        // Features expose icon names; the app must also register their SVG source.
        if let Some(bytes) = FolioIcons.load(path)? {
            return Ok(Some(bytes));
        }
        gpui_kit::assets::Assets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut paths = gpui_kit::assets::Assets.list(path)?;
        paths.extend(FolioIcons.list(path)?);
        for brand in [
            "brand/claude.svg",
            "brand/codex.svg",
            "brand/pi.svg",
            "brand/antigravity.svg",
        ] {
            if brand.starts_with(path) {
                paths.push(brand.into());
            }
        }
        paths.sort();
        paths.dedup();
        Ok(paths)
    }
}

pub fn icon(name: IconName) -> Icon {
    Icon::new(name).w(px(14.)).h(px(14.))
}

// ---------------------------------------------------------------------------
// Interaction feel
//
// Every clickable surface has three distinct states: rest, hover and pressed.
// The pressed state is what makes a click feel "solid": the surface darkens a
// step further, any outer lift shadow collapses and a faint inset shadow makes
// it read as pushed in. Keep new controls on these helpers instead of writing
// ad-hoc `.hover()` colours, otherwise the pressed feedback gets lost.
// ---------------------------------------------------------------------------

/// Neutral surface tints shared by ghost buttons, list rows and menu items.
pub const HOVER_BG: u32 = 0xefefed;
pub const PRESSED_BG: u32 = 0xe4e3df;

fn inset_press(alpha: f32) -> Vec<BoxShadow> {
    vec![BoxShadow::new(px(0.), px(1.), hsla(0., 0., 0., alpha)).blur_radius(px(2.)).inset()]
}

/// Raised look for bordered/filled buttons: a short drop shadow plus a hairline
/// top highlight so the surface reads as slightly convex.
fn raised(alpha: f32, highlight: f32) -> Vec<BoxShadow> {
    vec![
        BoxShadow::new(px(0.), px(1.), hsla(0., 0., 0., alpha)).blur_radius(px(2.)),
        BoxShadow::new(px(0.), px(1.), hsla(0., 0., 1., highlight)).inset(),
    ]
}

pub fn control(id: impl Into<ElementId>, name: IconName) -> Stateful<Div> {
    div()
        .id(id)
        .size(px(27.))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(5.))
        .cursor_pointer()
        .text_color(rgb(0x7d838b))
        .hover(|s| s.bg(rgb(0xe9ebed)).text_color(rgb(0x343a40)))
        .active(|s| {
            s.bg(rgb(0xdcdfe3))
                .text_color(rgb(0x1f2328))
                .shadow(inset_press(0.10))
        })
        .child(icon(name))
}

pub fn modal_overlay(
    title: impl IntoElement,
    description: impl IntoElement,
    actions: impl IntoElement,
) -> impl IntoElement {
    let card = div()
        .w(px(420.))
        .p_6()
        .rounded(px(10.))
        .border_1()
        .border_color(rgb(0xe6e5e1))
        .bg(rgb(0xffffff))
        .shadow(vec![
            BoxShadow::new(px(0.), px(16.), hsla(0., 0., 0., 0.16))
                .blur_radius(px(40.))
                .spread_radius(px(-8.)),
            BoxShadow::new(px(0.), px(2.), hsla(0., 0., 0., 0.06)).blur_radius(px(6.)),
        ])
        .flex()
        .flex_col()
        .gap_4()
        .child(div().font_semibold().child(title))
        .child(div().text_color(rgb(0x787774)).child(description))
        .child(actions)
        // Rise in from slightly below while fading in; feels anchored, not popped.
        .relative()
        .with_animation(
            "modal-card",
            Animation::new(std::time::Duration::from_millis(180)).with_easing(ease_out_quint()),
            |card, t| card.top(px(10. * (1. - t))).opacity(0.4 + 0.6 * t),
        );
    div()
        .id("modal-backdrop")
        .absolute()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        .bg(rgba(0x1c1b1838))
        // Keep hover / clicks from leaking to the UI behind the dialog.
        .occlude()
        .child(card)
        .with_animation(
            "modal-backdrop-fade",
            Animation::new(std::time::Duration::from_millis(140)).with_easing(ease_out_quint()),
            |backdrop, t| backdrop.opacity(t),
        )
}

fn button_base(id: impl Into<ElementId>, text: SharedString) -> Stateful<Div> {
    div()
        .id(id)
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .min_h(px(27.))
        .px_2()
        .py_1()
        .rounded(px(5.))
        .cursor_pointer()
        // Pass "" to build a custom body (icon + text) with `.child(...)`.
        .when(!text.is_empty(), |el| el.child(label(text)))
}

/// Ghost button: transparent at rest, tinted on hover, pushed-in on press.
pub fn button(id: impl Into<ElementId>, text: impl Into<SharedString>) -> Stateful<Div> {
    button_base(id, text.into())
        .text_color(rgb(0x37352f))
        .hover(|s| s.bg(rgb(HOVER_BG)))
        .active(|s| s.bg(rgb(PRESSED_BG)).shadow(inset_press(0.08)))
}

/// Bordered neutral button with a slight lift (e.g. "编译", "取消").
pub fn button_secondary(id: impl Into<ElementId>, text: impl Into<SharedString>) -> Stateful<Div> {
    button_base(id, text.into())
        .text_color(rgb(0x37352f))
        .bg(rgb(0xffffff))
        .border_1()
        .border_color(rgb(0xdcdbd7))
        .shadow(raised(0.06, 0.0))
        .hover(|s| s.bg(rgb(0xf7f7f5)).border_color(rgb(0xcfcec9)))
        .active(|s| {
            s.bg(rgb(0xedece9))
                .border_color(rgb(0xc5c4bf))
                .shadow(inset_press(0.10))
        })
}

/// Filled brand button for the main affirmative action.
pub fn button_primary(id: impl Into<ElementId>, text: impl Into<SharedString>) -> Stateful<Div> {
    button_base(id, text.into())
        .px_3()
        .font_medium()
        .text_color(crate::theme::on_accent())
        .bg(rgb(0x52769a))
        .border_1()
        .border_color(rgb(0x47698c))
        .shadow(raised(0.14, 0.14))
        .hover(|s| s.bg(rgb(0x486c90)).border_color(rgb(0x3f6082)))
        .active(|s| {
            s.bg(rgb(0x3d5d7e))
                .border_color(rgb(0x355373))
                .shadow(inset_press(0.22))
        })
}

/// Filled red button for destructive confirmations ("删除", "移除项目"...).
pub fn button_danger(id: impl Into<ElementId>, text: impl Into<SharedString>) -> Stateful<Div> {
    button_base(id, text.into())
        .px_3()
        .font_medium()
        .text_color(crate::theme::on_accent())
        .bg(rgb(0xd4373f))
        .border_1()
        .border_color(rgb(0xbf2f37))
        .shadow(raised(0.14, 0.14))
        .hover(|s| s.bg(rgb(0xc42f37)).border_color(rgb(0xae2830)))
        .active(|s| {
            s.bg(rgb(0xa9262e))
                .border_color(rgb(0x962128))
                .shadow(inset_press(0.25))
        })
}

/// Tinted red button for inline destructive actions in toolbars.
pub fn button_danger_soft(id: impl Into<ElementId>, text: impl Into<SharedString>) -> Stateful<Div> {
    button_base(id, text.into())
        .text_color(rgb(0xcf1322))
        .bg(rgb(0xfff1f0))
        .border_1()
        .border_color(rgb(0xffccc7))
        .shadow(raised(0.05, 0.0))
        .hover(|s| s.bg(rgb(0xffe4e1)).border_color(rgb(0xffa8a0)))
        .active(|s| {
            s.bg(rgb(0xffd3cd))
                .border_color(rgb(0xff8f85))
                .text_color(rgb(0xa8071a))
                .shadow(inset_press(0.10))
        })
}

pub const MAIN_TEX_COLOR: u32 = 0x5c7f9f;

pub fn file_icon(path: &str, asset: bool, main: bool) -> AnyElement {
    let lower = path.to_lowercase();
    if lower.ends_with(".pdf") {
        return Icon::default()
            .data(include_bytes!("../resources/icons/file-pdf.svg"))
            .w(px(16.))
            .h(px(18.))
            .text_color(rgb(0xd93025))
            .flex_shrink_0()
            .into_any_element();
    }
    if lower.ends_with(".html") || lower.ends_with(".htm") {
        return div()
            .flex_shrink_0()
            .text_color(rgb(0xd97706))
            .child(icon(IconName::Globe))
            .into_any_element();
    }
    let name = if asset {
        IconName::FileImage
    } else if lower.ends_with(".tex") {
        IconName::FileCode
    } else if lower.ends_with(".bib") {
        IconName::Library
    } else {
        IconName::FileText
    };
    div()
        .flex_shrink_0()
        .text_color(rgb(if main {
            MAIN_TEX_COLOR
        } else if lower.ends_with(".tex") {
            0x8b989f
        } else {
            0x787774
        }))
        .child(icon(name))
        .into_any_element()
}

pub fn label(text: SharedString) -> AnyElement {
    let (icon, caption) = match text.as_ref() {
        "×" => (IconName::Close, ""),
        "+" => (IconName::Plus, ""),
        "−" => (IconName::Minus, ""),
        "□" => (IconName::Maximize2, ""),
        "☰" => (IconName::PanelLeftClose, ""),
        "⌕" => (IconName::Search, ""),
        "▱" => (IconName::FolderOpen, ""),
        "◫" => (IconName::PanelRightOpen, ""),
        "☷" => (IconName::ListTree, ""),
        "▦" => (IconName::Table2, ""),
        "▧" => (IconName::Images, ""),
        "⌄" => (IconName::ChevronDown, ""),
        "源码" | "所见即所得" => (IconName::CodeXml, ""),
        "B" => (IconName::Bold, ""),
        "I" => (IconName::Italic, ""),
        "H" => (IconName::Heading2, ""),
        "•" => (IconName::List, ""),
        "☑" => (IconName::ListTodo, ""),
        "∑" => (IconName::Sigma, ""),
        "+  添加本地项目" => (IconName::Plus, "添加本地项目"),
        "‹" => (IconName::ChevronLeft, ""),
        "›" => (IconName::ChevronRight, ""),
        "↓" => (IconName::ArrowDown, ""),
        "✓" => (IconName::Check, ""),
        "📁+" => (IconName::FolderPlus, ""),
        "导出" => (IconName::Download, ""),
        "⚙  设置" => (IconName::Settings, "设置"),
        "终端" => (IconName::SquareTerminal, ""),
        "▶  编译" => (IconName::Play, ""),
        "■  停止" => (IconName::Square, ""),
        _ => return div().child(text).into_any_element(),
    };
    div()
        .flex()
        .items_center()
        .gap_1()
        .flex_shrink_0()
        .child(self::icon(icon))
        .children((!caption.is_empty()).then_some(caption))
        .into_any_element()
}
