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

pub const DEFAULT_SANS_FONT: &str = "Segoe UI";

pub fn sans_font(family: &str) -> SharedString {
    if family.trim().is_empty() {
        DEFAULT_SANS_FONT.into()
    } else {
        family.trim().to_owned().into()
    }
}

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
            .tooltip(if copied {
                crate::i18n::common::COPIED_FULL_TEXT.get()
            } else {
                crate::i18n::common::COPY_FULL_TEXT.get()
            })
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

pub fn inline_code_style(settings: &crate::model::AppSettings, body_size: Pixels) -> StyleRefinement {
    let mut style = StyleRefinement::default().text_size(body_size * settings.inline_code_scale);
    if settings.theme.palette != crate::theme::Palette::light() {
        style = style.bg(gpui_kit::rgb(settings.theme.palette.code_background));
    }
    if !settings.inline_code_font.is_empty() {
        style = style.font_family(settings.inline_code_font.clone());
    }
    style
}

/// Shared corner radius for Markdown block containers (code blocks, tables),
/// aligned with the WYSIWYG Markdown editor (`gpui-kit-markdown-editor`).
pub const MD_BLOCK_RADIUS: Pixels = px(8.);

pub fn block_code_style(settings: &crate::model::AppSettings, body_size: Pixels) -> StyleRefinement {
    let mut style = StyleRefinement::default()
        .text_size(body_size * settings.block_code_scale)
        .rounded(MD_BLOCK_RADIUS);
    if settings.theme.palette != crate::theme::Palette::light() {
        style = style.bg(gpui_kit::rgb(settings.theme.palette.code_background));
    }
    if !settings.block_code_font.is_empty() {
        style = style.font_family(settings.block_code_font.clone());
    }
    style
}

pub fn markdown_table_style() -> StyleRefinement {
    StyleRefinement::default().rounded(MD_BLOCK_RADIUS)
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
        ChartNetwork,
        PencilLine,
        Pin,
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
        Tag,
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

const BRAND_AI_MASCOT_PNG: &[u8] = include_bytes!("../resources/icons/ai-mascot.png");

const BRAND_CLAUDE_SVG: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="48" height="48" viewBox="0 0 24 24" fill="#D97757"><path d="m4.7144 15.9555 4.7174-2.6471.079-.2307-.079-.1275h-.2307l-.7893-.0486-2.6956-.0729-2.3375-.0971-2.2646-.1214-.5707-.1215-.5343-.7042.0546-.3522.4797-.3218.686.0608 1.5179.1032 2.2767.1578 1.6514.0972 2.4468.255h.3886l.0546-.1579-.1336-.0971-.1032-.0972L6.973 9.8356l-2.55-1.6879-1.3356-.9714-.7225-.4918-.3643-.4614-.1578-1.0078.6557-.7225.8803.0607.2246.0607.8925.686 1.9064 1.4754 2.4893 1.8336.3643.3035.1457-.1032.0182-.0728-.164-.2733-1.3539-2.4467-1.445-2.4893-.6435-1.032-.17-.6194c-.0607-.255-.1032-.4674-.1032-.7285L6.287.1335 6.6997 0l.9957.1336.419.3642.6192 1.4147 1.0018 2.2282 1.5543 3.0296.4553.8985.2429.8318.091.255h.1579v-.1457l.1275-1.706.2368-2.0947.2307-2.6957.0789-.7589.3764-.9107.7468-.4918.5828.2793.4797.686-.0668.4433-.2853 1.8517-.5586 2.9021-.3643 1.9429h.2125l.2429-.2429.9835-1.3053 1.6514-2.0643.7286-.8196.85-.9046.5464-.4311h1.0321l.759 1.1293-.34 1.1657-1.0625 1.3478-.8804 1.1414-1.2628 1.7-.7893 1.36.0729.1093.1882-.0183 2.8535-.607 1.5421-.2794 1.8396-.3157.8318.3886.091.3946-.3278.8075-1.967.4857-2.3072.4614-3.4364.8136-.0425.0304.0486.0607 1.5482.1457.6618.0364h1.621l3.0175.2247.7892.522.4736.6376-.079.4857-1.2142.6193-1.6393-.3886-3.825-.9107-1.3113-.3279h-.1822v.1093l1.0929 1.0686 2.0035 1.8092 2.5075 2.3314.1275.5768-.3218.4554-.34-.0486-2.2039-1.6575-.85-.7468-1.9246-1.621h-.1275v.17l.4432.6496 2.3436 3.5214.1214 1.0807-.17.3521-.6071.2125-.6679-.1214-1.3721-1.9246L14.38 17.959l-1.1414-1.9428-.1397.079-.674 7.2552-.3156.3703-.7286.2793-.6071-.4614-.3218-.7468.3218-1.4753.3886-1.9246.3157-1.53.2853-1.9004.17-.6314-.0121-.0425-.1397.0182-1.4328 1.9672-2.1796 2.9446-1.7243 1.8456-.4128.164-.7164-.3704.0667-.6618.4008-.5889 2.386-3.0357 1.4389-1.882.929-1.0868-.0062-.1579h-.0546l-6.3385 4.1164-1.1293.1457-.4857-.4554.0608-.7467.2307-.2429 1.9064-1.3114Z"/></svg>"##;

const BRAND_CODEX_SVG: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="48" height="48" viewBox="0 0 24 24" fill="currentColor" fill-rule="evenodd"><path d="M9.205 8.658v-2.26c0-.19.072-.333.238-.428l4.543-2.616c.619-.357 1.356-.523 2.117-.523 2.854 0 4.662 2.212 4.662 4.566 0 .167 0 .357-.024.547l-4.71-2.759a.797.797 0 00-.856 0l-5.97 3.473zm10.609 8.8V12.06c0-.333-.143-.57-.429-.737l-5.97-3.473 1.95-1.118a.433.433 0 01.476 0l4.543 2.617c1.309.76 2.189 2.378 2.189 3.948 0 1.808-1.07 3.473-2.76 4.163zM7.802 12.703l-1.95-1.142c-.167-.095-.239-.238-.239-.428V5.899c0-2.545 1.95-4.472 4.591-4.472 1 0 1.927.333 2.712.928L8.23 5.067c-.285.166-.428.404-.428.737v6.898zM12 15.128l-2.795-1.57v-3.33L12 8.658l2.795 1.57v3.33L12 15.128zm1.796 7.23c-1 0-1.927-.332-2.712-.927l4.686-2.712c.285-.166.428-.404.428-.737v-6.898l1.974 1.142c.167.095.238.238.238.428v5.233c0 2.545-1.974 4.472-4.614 4.472zm-5.637-5.303l-4.544-2.617c-1.308-.761-2.188-2.378-2.188-3.948A4.482 4.482 0 014.21 6.327v5.423c0 .333.143.571.428.738l5.947 3.449-1.95 1.118a.432.432 0 01-.476 0zm-.262 3.9c-2.688 0-4.662-2.021-4.662-4.519 0-.19.024-.38.047-.57l4.686 2.71c.286.167.571.167.856 0l5.97-3.448v2.26c0 .19-.07.333-.237.428l-4.543 2.616c-.619.357-1.356.523-2.117.523zm5.899 2.83a5.947 5.947 0 005.827-4.756C22.287 18.339 24 15.84 24 13.296c0-1.665-.713-3.282-1.998-4.448.119-.5.19-.999.19-1.498 0-3.401-2.759-5.947-5.946-5.947-.642 0-1.26.095-1.88.31A5.962 5.962 0 0010.205 0a5.947 5.947 0 00-5.827 4.757C1.713 5.447 0 7.945 0 10.49c0 1.666.713 3.283 1.998 4.448-.119.5-.19 1-.19 1.499 0 3.401 2.759 5.946 5.946 5.946.642 0 1.26-.095 1.88-.309a5.96 5.96 0 004.162 1.713z"/></svg>"##;

const BRAND_HUNYUAN_SVG: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="48" height="48" viewBox="0 0 24 24"><circle cx="12" cy="12" fill="#0055E9" r="12"/><path d="M12 0c.518 0 1.028.033 1.528.096A6.188 6.188 0 0112.12 12.28l-.12.001c-2.99 0-5.242 2.179-5.554 5.11-.223 2.086.353 4.412 2.242 6.146C3.672 22.1 0 17.479 0 12 0 5.373 5.373 0 12 0z" fill="#A8DFF5"/><path d="M5.286 5a2.438 2.438 0 01.682 3.38c-3.962 5.966-3.215 10.743 2.648 15.136C3.636 22.056 0 17.452 0 12c0-1.787.39-3.482 1.09-5.006.253-.435.525-.872.817-1.311A2.438 2.438 0 015.286 5z" fill="#0055E9"/><path d="M12.98.04c.272.021.543.053.81.093.583.106 1.117.254 1.538.44 6.638 2.927 8.07 10.052 1.748 15.642a4.125 4.125 0 01-5.822-.358c-1.51-1.706-1.3-4.184.357-5.822.858-.848 3.108-1.223 4.045-2.441 1.257-1.634 2.122-6.009-2.523-7.506L12.98.039z" fill="#00BCFF"/><path d="M13.528.096A6.187 6.187 0 0112 12.281a5.75 5.75 0 00-1.71.255c.147-.905.595-1.784 1.321-2.501.858-.848 3.108-1.223 4.045-2.441 1.27-1.651 2.14-6.104-2.676-7.554.184.014.367.033.548.056z" fill="#ECECEE"/></svg>"##;

const BRAND_PI_SVG: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="48" height="48" viewBox="0 0 800 800"><path fill="#F09082" d="M165.29 165.29H517.36V400H400V282.65H165.29Z"/><path fill="#4D9ABF" d="M165.29 282.65H282.65V400H400V517.36H282.65V634.72H165.29Z"/><path fill="#F1BE58" d="M517.36 400H634.72V634.72H517.36Z"/></svg>"##;

const BRAND_ANTIGRAVITY_SVG: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="48" height="48" viewBox="0 0 24 24"><defs><clipPath id="ag-clip"><path d="M21.751 22.607c1.34 1.005 3.35.335 1.508-1.508C17.73 15.74 18.904 1 12.037 1 5.17 1 6.342 15.74.815 21.1c-2.01 2.009.167 2.511 1.507 1.506 5.192-3.517 4.857-9.714 9.715-9.714 4.857 0 4.522 6.197 9.714 9.715z"/></clipPath></defs><g clip-path="url(#ag-clip)"><rect width="24" height="24" fill="#3186FF"/><path d="M-1.018-3.992c-.408 3.591 2.686 6.89 6.91 7.37 4.225.48 7.98-2.043 8.387-5.633.408-3.59-2.686-6.89-6.91-7.37-4.225-.479-7.98 2.043-8.387 5.633z" fill="#FFE432"/><path d="M15.269 7.747c1.058 4.557 5.691 7.374 10.348 6.293 4.657-1.082 7.575-5.653 6.516-10.21-1.058-4.556-5.691-7.374-10.348-6.292-4.657 1.082-7.575 5.653-6.516 10.21z" fill="#FC413D"/><path d="M-7.608 14.703c3.352 3.424 9.126 3.208 12.896-.483 3.77-3.69 4.108-9.459.756-12.883C2.69-2.087-3.083-1.871-6.853 1.82c-3.77 3.69-4.108 9.458-.755 12.883z" fill="#00B95C"/><path d="M9.932 27.617c1.04 4.482 5.384 7.303 9.7 6.3 4.316-1.002 6.971-5.448 5.93-9.93-1.04-4.483-5.384-7.304-9.7-6.301-4.316 1.002-6.971 5.448-5.93 9.93z" fill="#3186FF"/><path d="M18.163 9.077c5.81 3.93 12.502 4.19 14.946.577 2.443-3.612-.287-9.727-6.098-13.658-5.81-3.931-12.502-4.19-14.946-.577-2.443 3.612.287 9.727 6.098 13.658z" fill="#FC413D"/><path d="M-.915 2.684c-1.44 3.473-.97 6.967 1.05 7.804 2.02.837 4.824-1.3 6.264-4.772 1.44-3.473.97-6.967-1.05-7.804-2.02-.837-4.824 1.3-6.264 4.772z" fill="#FFEE48"/></g></svg>"##;

const BRAND_GEMINI_SVG: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="48" height="48" viewBox="0 0 24 24"><path d="M20.616 10.835a14.147 14.147 0 01-4.45-3.001 14.111 14.111 0 01-3.678-6.452.503.503 0 00-.975 0 14.134 14.134 0 01-3.679 6.452 14.155 14.155 0 01-4.45 3.001c-.65.28-1.318.505-2.002.678a.502.502 0 000 .975c.684.172 1.35.397 2.002.677a14.147 14.147 0 014.45 3.001 14.112 14.112 0 013.679 6.453.502.502 0 00.975 0c.172-.685.397-1.351.677-2.003a14.145 14.145 0 013.001-4.45 14.113 14.113 0 016.453-3.678.503.503 0 000-.975 13.245 13.245 0 01-2.003-.678z" fill="#3186FF"/><path d="M20.616 10.835a14.147 14.147 0 01-4.45-3.001 14.111 14.111 0 01-3.678-6.452.503.503 0 00-.975 0 14.134 14.134 0 01-3.679 6.452 14.155 14.155 0 01-4.45 3.001c-.65.28-1.318.505-2.002.678a.502.502 0 000 .975c.684.172 1.35.397 2.002.677a14.147 14.147 0 014.45 3.001 14.112 14.112 0 013.679 6.453.502.502 0 00.975 0c.172-.685.397-1.351.677-2.003a14.145 14.145 0 013.001-4.45 14.113 14.113 0 016.453-3.678.503.503 0 000-.975 13.245 13.245 0 01-2.003-.678z" fill="url(#gem-0)"/><path d="M20.616 10.835a14.147 14.147 0 01-4.45-3.001 14.111 14.111 0 01-3.678-6.452.503.503 0 00-.975 0 14.134 14.134 0 01-3.679 6.452 14.155 14.155 0 01-4.45 3.001c-.65.28-1.318.505-2.002.678a.502.502 0 000 .975c.684.172 1.35.397 2.002.677a14.147 14.147 0 014.45 3.001 14.112 14.112 0 013.679 6.453.502.502 0 00.975 0c.172-.685.397-1.351.677-2.003a14.145 14.145 0 013.001-4.45 14.113 14.113 0 016.453-3.678.503.503 0 000-.975 13.245 13.245 0 01-2.003-.678z" fill="url(#gem-1)"/><path d="M20.616 10.835a14.147 14.147 0 01-4.45-3.001 14.111 14.111 0 01-3.678-6.452.503.503 0 00-.975 0 14.134 14.134 0 01-3.679 6.452 14.155 14.155 0 01-4.45 3.001c-.65.28-1.318.505-2.002.678a.502.502 0 000 .975c.684.172 1.35.397 2.002.677a14.147 14.147 0 014.45 3.001 14.112 14.112 0 013.679 6.453.502.502 0 00.975 0c.172-.685.397-1.351.677-2.003a14.145 14.145 0 013.001-4.45 14.113 14.113 0 016.453-3.678.503.503 0 000-.975 13.245 13.245 0 01-2.003-.678z" fill="url(#gem-2)"/><defs><linearGradient gradientUnits="userSpaceOnUse" id="gem-0" x1="7" x2="11" y1="15.5" y2="12"><stop stop-color="#08B962"/><stop offset="1" stop-color="#08B962" stop-opacity="0"/></linearGradient><linearGradient gradientUnits="userSpaceOnUse" id="gem-1" x1="8" x2="11.5" y1="5.5" y2="11"><stop stop-color="#F94543"/><stop offset="1" stop-color="#F94543" stop-opacity="0"/></linearGradient><linearGradient gradientUnits="userSpaceOnUse" id="gem-2" x1="3.5" x2="17.5" y1="13.5" y2="12"><stop stop-color="#FABC12"/><stop offset=".46" stop-color="#FABC12" stop-opacity="0"/></linearGradient></defs></svg>"##;

const BRAND_DEEPSEEK_SVG: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="48" height="48" viewBox="0 0 24 24"><path d="M23.748 4.482c-.254-.124-.364.113-.512.234-.051.039-.094.09-.137.136-.372.397-.806.657-1.373.626-.829-.046-1.537.214-2.163.848-.133-.782-.575-1.248-1.247-1.548-.352-.156-.708-.311-.955-.65-.172-.241-.219-.51-.305-.774-.055-.16-.11-.323-.293-.35-.2-.031-.278.136-.356.276-.313.572-.434 1.202-.422 1.84.027 1.436.633 2.58 1.838 3.393.137.093.172.187.129.323-.082.28-.18.552-.266.833-.055.179-.137.217-.329.14a5.526 5.526 0 01-1.736-1.18c-.857-.828-1.631-1.742-2.597-2.458a11.365 11.365 0 00-.689-.471c-.985-.957.13-1.743.388-1.836.27-.098.093-.432-.779-.428-.872.004-1.67.295-2.687.684a3.055 3.055 0 01-.465.137 9.597 9.597 0 00-2.883-.102c-1.885.21-3.39 1.102-4.497 2.623C.082 8.606-.231 10.684.152 12.85c.403 2.284 1.569 4.175 3.36 5.653 1.858 1.533 3.997 2.284 6.438 2.14 1.482-.085 3.133-.284 4.994-1.86.47.234.962.327 1.78.397.63.059 1.236-.03 1.705-.128.735-.156.684-.837.419-.961-2.155-1.004-1.682-.595-2.113-.926 1.096-1.296 2.746-2.642 3.392-7.003.05-.347.007-.565 0-.845-.004-.17.035-.237.23-.256a4.173 4.173 0 001.545-.475c1.396-.763 1.96-2.015 2.093-3.517.02-.23-.004-.467-.247-.588zM11.581 18c-2.089-1.642-3.102-2.183-3.52-2.16-.392.024-.321.471-.235.763.09.288.207.486.371.739.114.167.192.416-.113.603-.673.416-1.842-.14-1.897-.167-1.361-.802-2.5-1.86-3.301-3.307-.774-1.393-1.224-2.887-1.298-4.482-.02-.386.093-.522.477-.592a4.696 4.696 0 011.529-.039c2.132.312 3.946 1.265 5.468 2.774.868.86 1.525 1.887 2.202 2.891.72 1.066 1.494 2.082 2.48 2.914.348.292.625.514.891.677-.802.09-2.14.11-3.054-.614zm1-6.44a.306.306 0 01.415-.287.302.302 0 01.2.288.306.306 0 01-.31.307.303.303 0 01-.304-.308zm3.11 1.596c-.2.081-.399.151-.59.16a1.245 1.245 0 01-.798-.254c-.274-.23-.47-.358-.552-.758a1.73 1.73 0 01.016-.588c.07-.327-.008-.537-.239-.727-.187-.156-.426-.199-.688-.199a.559.559 0 01-.254-.078c-.11-.054-.2-.19-.114-.358.028-.054.16-.186.192-.21.356-.202.767-.136 1.146.016.352.144.618.408 1.001.782.391.451.462.576.685.914.176.265.336.537.445.848.067.195-.019.354-.25.452z" fill="#4D6BFE"/></svg>"##;

const BRAND_QWEN_SVG: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="48" height="48" viewBox="0 0 24 24"><path d="M19.713 2.825h-5.907c0-.251-.066-.503-.196-.728L12.819.728A1.46 1.46 0 0 0 11.559 0h-1.58c-.52 0-1 .275-1.26.728l-2.42 4.19a1.45 1.45 0 0 0 0 1.453l.791 1.37c.26.448.74.727 1.26.727h11.36c.52 0 1-.275 1.26-.728l.79-1.37a1.45 1.45 0 0 0 0-1.451l-.79-1.37a1.46 1.46 0 0 0-1.26-.728zM.195 9.905l2.951 5.115a1.46 1.46 0 0 0-.531.531l-.791 1.37a1.45 1.45 0 0 0 0 1.452l.79 1.37c.26.448.74.727 1.26.727h4.84c.52 0 1-.275 1.26-.728l.79-1.37a1.45 1.45 0 0 0 0-1.451L5.086 7.083a1.46 1.46 0 0 0-1.26-.728h-1.58c-.52 0-1 .276-1.26.728l-.79 1.37a1.45 1.45 0 0 0 0 1.452m15.888 13.367 2.951-5.115c.217.125.469.196.728.196h1.582c.52 0 1-.275 1.26-.728l.79-1.37a1.45 1.45 0 0 0 0-1.451l-2.42-4.191a1.46 1.46 0 0 0-1.259-.728h-1.582c-.52 0-1 .276-1.259.728l-5.678 9.838a1.45 1.45 0 0 0 0 1.452l.79 1.37c.26.448.74.727 1.26.727h1.582c.52 0 1-.275 1.26-.728z" fill="#615CED"/></svg>"##;

const BRAND_ZHIPU_SVG: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="48" height="48" viewBox="0 0 24 24"><path d="M11.991 23.503a.24.24 0 00-.244.248.24.24 0 00.244.249.24.24 0 00.245-.249.24.24 0 00-.22-.247l-.025-.001zM9.671 5.365a1.697 1.697 0 011.099 2.132l-.071.172-.016.04-.018.054c-.07.16-.104.32-.104.498-.035.71.47 1.279 1.186 1.314h.366c1.309.053 2.338 1.173 2.286 2.523-.052 1.332-1.152 2.38-2.478 2.327h-.174c-.715.018-1.274.64-1.239 1.368 0 .124.018.23.053.337.209.373.54.658.96.8.75.23 1.517-.125 1.9-.782l.018-.035c.402-.64 1.17-.96 1.92-.711.854.284 1.378 1.226 1.099 2.167a1.661 1.661 0 01-2.077 1.102 1.711 1.711 0 01-.907-.711l-.017-.035c-.2-.323-.463-.58-.851-.711l-.056-.018a1.646 1.646 0 00-1.954.746 1.66 1.66 0 01-1.065.764 1.677 1.677 0 01-1.989-1.279c-.209-.906.332-1.83 1.257-2.043a1.51 1.51 0 01.296-.035h.018c.68-.071 1.151-.622 1.116-1.333a1.307 1.307 0 00-.227-.693 2.515 2.515 0 01-.366-1.403 2.39 2.39 0 01.366-1.208c.14-.195.21-.444.227-.693.018-.71-.506-1.261-1.186-1.332l-.07-.018a1.43 1.43 0 01-.299-.07l-.05-.019a1.7 1.7 0 01-1.047-2.114 1.68 1.68 0 012.094-1.101zm-5.575 10.11c.26-.264.639-.367.994-.27.355.096.633.379.728.74.095.362-.007.748-.267 1.013-.402.41-1.053.41-1.455 0a1.062 1.062 0 010-1.482zm14.845-.294c.359-.09.738.024.992.297.254.274.344.665.237 1.025-.107.36-.396.634-.756.718-.551.128-1.1-.22-1.23-.781a1.05 1.05 0 01.757-1.26zm-.064-4.39c.314.32.49.753.49 1.206 0 .452-.176.886-.49 1.206-.315.32-.74.5-1.185.5-.444 0-.87-.18-1.184-.5a1.727 1.727 0 010-2.412 1.654 1.654 0 012.369 0zm-11.243.163c.364.484.447 1.128.218 1.691a1.665 1.665 0 01-2.188.923c-.855-.36-1.26-1.358-.907-2.228a1.68 1.68 0 011.33-1.038c.593-.08 1.183.169 1.547.652zm11.545-4.221c.368 0 .708.2.892.524.184.324.184.724 0 1.048a1.026 1.026 0 01-.892.524c-.568 0-1.03-.47-1.03-1.048 0-.579.462-1.048 1.03-1.048zm-14.358 0c.368 0 .707.2.891.524.184.324.184.724 0 1.048a1.026 1.026 0 01-.891.524c-.569 0-1.03-.47-1.03-1.048 0-.579.461-1.048 1.03-1.048zm10.031-1.475c.925 0 1.675.764 1.675 1.706s-.75 1.705-1.675 1.705-1.674-.763-1.674-1.705c0-.942.75-1.706 1.674-1.706zm-2.626-.684c.362-.082.653-.356.761-.718a1.062 1.062 0 00-.238-1.028 1.017 1.017 0 00-.996-.294c-.547.14-.881.7-.752 1.257.13.558.675.907 1.225.783zm0 16.876c.359-.087.644-.36.75-.72a1.062 1.062 0 00-.237-1.019 1.018 1.018 0 00-.985-.301 1.037 1.037 0 00-.762.717c-.108.361-.017.754.239 1.028.245.263.606.377.953.305l.043-.01zM17.19 3.5a.631.631 0 00.628-.64c0-.355-.279-.64-.628-.64a.631.631 0 00-.628.64c0 .355.28.64.628.64zm-10.38 0a.631.631 0 00.628-.64c0-.355-.28-.64-.628-.64a.631.631 0 00-.628.64c0 .355.279.64.628.64zm-5.182 7.852a.631.631 0 00-.628.64c0 .354.28.639.628.639a.63.63 0 00.627-.606l.001-.034a.62.62 0 00-.628-.64zm5.182 9.13a.631.631 0 00-.628.64c0 .355.279.64.628.64a.631.631 0 00.628-.64c0-.355-.28-.64-.628-.64zm10.38.018a.631.631 0 00-.628.64c0 .355.28.64.628.64a.631.631 0 00.628-.64c0-.355-.279-.64-.628-.64zm5.182-9.148a.631.631 0 00-.628.64c0 .354.279.639.628.639a.631.631 0 00.628-.64c0-.355-.28-.64-.628-.64zm-.384-4.992a.24.24 0 00.244-.249.24.24 0 00-.244-.249.24.24 0 00-.244.249c0 .142.122.249.244.249zM11.991.497a.24.24 0 00.245-.248A.24.24 0 0011.99 0a.24.24 0 00-.244.249c0 .133.108.236.223.247l.021.001zM2.011 6.36a.24.24 0 00.245-.249.24.24 0 00-.244-.249.24.24 0 00-.244.249.24.24 0 00.244.249zm0 11.263a.24.24 0 00-.243.248.24.24 0 00.244.249.24.24 0 00.244-.249.252.252 0 00-.244-.248zm19.995-.018a.24.24 0 00-.245.248.24.24 0 00.245.25.24.24 0 00.244-.25.252.252 0 00-.244-.248z" fill="#3859FF" fill-rule="nonzero"/></svg>"##;

const BRAND_GROK_SVG: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="48" height="48" viewBox="0 0 24 24" fill="#64748b" fill-rule="evenodd"><path d="M9.27 15.29l7.978-5.897c.391-.29.95-.177 1.137.272.98 2.369.542 5.215-1.41 7.169-1.951 1.954-4.667 2.382-7.149 1.406l-2.711 1.257c3.889 2.661 8.611 2.003 11.562-.953 2.341-2.344 3.066-5.539 2.388-8.42l.006.007c-.983-4.232.242-5.924 2.75-9.383.06-.082.12-.164.179-.248l-3.301 3.305v-.01L9.267 15.292M7.623 16.723c-2.792-2.67-2.31-6.801.071-9.184 1.761-1.763 4.647-2.483 7.166-1.425l2.705-1.25a7.808 7.808 0 00-1.829-1A8.975 8.975 0 005.984 5.83c-2.533 2.536-3.33 6.436-1.962 9.764 1.022 2.487-.653 4.246-2.34 6.022-.599.63-1.199 1.259-1.682 1.925l7.62-6.815"/></svg>"##;

const BRAND_KIMI_SVG: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="48" height="48" viewBox="0 0 24 24"><path d="M21.846 0a1.923 1.923 0 110 3.846H20.15a.226.226 0 01-.227-.226V1.923C19.923.861 20.784 0 21.846 0z" fill="#1783FF"/><path d="M11.065 11.199l7.257-7.2c.137-.136.06-.41-.116-.41H14.3a.164.164 0 00-.117.051l-7.82 7.756c-.122.12-.302.013-.302-.179V3.82c0-.127-.083-.23-.185-.23H3.186c-.103 0-.186.103-.186.23V19.77c0 .128.083.23.186.23h2.69c.103 0 .186-.102.186-.23v-3.25c0-.069.025-.135.069-.178l2.424-2.406a.158.158 0 01.205-.023l6.484 4.772a7.677 7.677 0 003.453 1.283c.108.012.2-.095.2-.23v-3.06c0-.117-.07-.212-.164-.227a5.028 5.028 0 01-2.027-.807l-5.613-4.064c-.117-.078-.132-.279-.028-.381z" fill="#475569"/></svg>"##;

const BRAND_MINIMAX_SVG: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="48" height="48" viewBox="0 0 24 24"><defs><linearGradient id="mm-0" x1="0%" x2="100.182%" y1="50.057%" y2="50.057%"><stop offset="0%" stop-color="#E2167E"/><stop offset="100%" stop-color="#FE603C"/></linearGradient></defs><path d="M16.278 2c1.156 0 2.093.927 2.093 2.07v12.501a.74.74 0 00.744.709.74.74 0 00.743-.709V9.099a2.06 2.06 0 012.071-2.049A2.06 2.06 0 0124 9.1v6.561a.649.649 0 01-.652.645.649.649 0 01-.653-.645V9.1a.762.762 0 00-.766-.758.762.762 0 00-.766.758v7.472a2.037 2.037 0 01-2.048 2.026 2.037 2.037 0 01-2.048-2.026v-12.5a.785.785 0 00-.788-.753.785.785 0 00-.789.752l-.001 15.904A2.037 2.037 0 0113.441 22a2.037 2.037 0 01-2.048-2.026V18.04c0-.356.292-.645.652-.645.36 0 .652.289.652.645v1.934c0 .263.142.506.372.638.23.131.514.131.744 0a.734.734 0 00.372-.638V4.07c0-1.143.937-2.07 2.093-2.07zm-5.674 0c1.156 0 2.093.927 2.093 2.07v11.523a.648.648 0 01-.652.645.648.648 0 01-.652-.645V4.07a.785.785 0 00-.789-.78.785.785 0 00-.789.78v14.013a2.06 2.06 0 01-2.07 2.048 2.06 2.06 0 01-2.071-2.048V9.1a.762.762 0 00-.766-.758.762.762 0 00-.766.758v3.8a2.06 2.06 0 01-2.071 2.049A2.06 2.06 0 010 12.9v-1.378c0-.357.292-.646.652-.646.36 0 .653.29.653.646V12.9c0 .418.343.757.766.757s.766-.339.766-.757V9.099a2.06 2.06 0 012.07-2.048 2.06 2.06 0 012.071 2.048v8.984c0 .419.343.758.767.758.423 0 .766-.339.766-.758V4.07c0-1.143.937-2.07 2.093-2.07z" fill="url(#mm-0)" fill-rule="nonzero"/></svg>"##;

const BRAND_MISTRAL_SVG: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="48" height="48" viewBox="0 0 24 24"><path d="M3.428 3.4h3.429v3.428H3.428V3.4zm13.714 0h3.43v3.428h-3.43V3.4z" fill="#FFD700"/><path d="M3.428 6.828h6.857v3.429H3.429V6.828zm10.286 0h6.857v3.429h-6.857V6.828z" fill="#FFAF00"/><path d="M3.428 10.258h17.144v3.428H3.428v-3.428z" fill="#FF8205"/><path d="M3.428 13.686h3.429v3.428H3.428v-3.428zm6.858 0h3.429v3.428h-3.429v-3.428zm6.856 0h3.43v3.428h-3.43v-3.428z" fill="#FA500F"/><path d="M0 17.114h10.286v3.429H0v-3.429zm13.714 0H24v3.429H13.714v-3.429z" fill="#E10500"/></svg>"##;

const BRAND_META_SVG: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="48" height="48" viewBox="0 0 24 24"><path d="M6.897 4h-.024l-.031 2.615h.022c1.715 0 3.046 1.357 5.94 6.246l.175.297.012.02 1.62-2.438-.012-.019a48.763 48.763 0 00-1.098-1.716 28.01 28.01 0 00-1.175-1.629C10.413 4.932 8.812 4 6.896 4z" fill="#0867DF"/><path d="M6.873 4C4.95 4.01 3.247 5.258 2.02 7.17a4.352 4.352 0 00-.01.017l2.254 1.231.011-.017c.718-1.083 1.61-1.774 2.568-1.785h.021L6.896 4h-.023z" fill="#0064E0"/><path d="M2.019 7.17l-.011.017C1.2 8.447.598 9.995.274 11.664l-.005.022 2.534.6.004-.022c.27-1.467.786-2.828 1.456-3.845l.011-.017L2.02 7.17z" fill="#0072EC"/><path d="M2.807 12.264l-2.533-.6-.005.022c-.177.918-.267 1.851-.269 2.786v.023l2.598.233v-.023a12.591 12.591 0 01.21-2.44z" fill="#007CF6"/><path d="M2.677 15.537a5.462 5.462 0 01-.079-.813v-.022L0 14.468v.024a8.89 8.89 0 00.146 1.652l2.535-.585a4.106 4.106 0 01-.004-.022z" fill="#007FF9"/><path d="M3.27 16.89c-.284-.31-.484-.756-.589-1.328l-.004-.021-2.535.585.004.021c.192 1.01.568 1.85 1.106 2.487l.014.017 2.018-1.745a2.106 2.106 0 01-.015-.016z" fill="#0082FB"/><path d="M10.78 9.654c-1.528 2.35-2.454 3.825-2.454 3.825-2.035 3.2-2.739 3.917-3.871 3.917a1.545 1.545 0 01-1.186-.508l-2.017 1.744.014.017C2.01 19.518 3.058 20 4.356 20c1.963 0 3.374-.928 5.884-5.33l1.766-3.13a41.283 41.283 0 00-1.227-1.886z" fill="#0082FB"/><path d="M13.502 5.946l-.016.016c-.4.43-.786.908-1.16 1.416.378.483.768 1.024 1.175 1.63.48-.743.928-1.345 1.367-1.807l.016-.016-1.382-1.24z" fill="#007FF8"/><path d="M20.918 5.713C19.853 4.633 18.583 4 17.225 4c-1.432 0-2.637.787-3.723 1.944l-.016.016 1.382 1.24.016-.017c.715-.747 1.408-1.12 2.176-1.12.826 0 1.6.39 2.27 1.075l.015.016 1.589-1.425-.016-.016z" fill="#0082FB"/><path d="M23.998 14.125c-.06-3.467-1.27-6.566-3.064-8.396l-.016-.016-1.588 1.424.015.016c1.35 1.392 2.277 3.98 2.361 6.971v.023h2.292v-.022z" fill="#0081FA"/><path d="M23.998 14.15v-.023h-2.292v.022c.004.14.006.282.006.424 0 .815-.121 1.474-.368 1.95l-.011.022 1.708 1.782.013-.02c.62-.96.946-2.293.946-3.91 0-.083 0-.165-.002-.247z" fill="#0080F9"/><path d="M21.344 16.52l-.011.02c-.214.402-.519.67-.917.787l.778 2.462a3.493 3.493 0 00.438-.182 3.558 3.558 0 001.366-1.218l.044-.065.012-.02-1.71-1.784z" fill="#027AF3"/><path d="M19.92 17.393c-.262 0-.492-.039-.718-.14l-.798 2.522c.449.153.927.222 1.46.222.492 0 .943-.073 1.352-.215l-.78-2.462c-.167.05-.341.075-.517.073z" fill="#0377EF"/><path d="M18.323 16.534l-.014-.017-1.836 1.914.016.017c.637.682 1.246 1.105 1.937 1.337l.797-2.52c-.291-.125-.573-.353-.9-.731z" fill="#0471E9"/><path d="M18.309 16.515c-.55-.642-1.232-1.712-2.303-3.44l-1.396-2.336-.011-.02-1.62 2.438.012.02.989 1.668c.959 1.61 1.74 2.774 2.493 3.585l.016.016 1.834-1.914a2.353 2.353 0 01-.014-.017z" fill="#0867DF"/></svg>"##;

pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        match path {
            "brand/ai-mascot.png" => return Ok(Some(Cow::Borrowed(BRAND_AI_MASCOT_PNG))),
            "brand/claude.svg" => return Ok(Some(Cow::Borrowed(BRAND_CLAUDE_SVG))),
            "brand/codex.svg" => return Ok(Some(Cow::Borrowed(BRAND_CODEX_SVG))),
            "brand/pi.svg" => return Ok(Some(Cow::Borrowed(BRAND_PI_SVG))),
            "brand/antigravity.svg" => return Ok(Some(Cow::Borrowed(BRAND_ANTIGRAVITY_SVG))),
            "brand/gemini.svg" => return Ok(Some(Cow::Borrowed(BRAND_GEMINI_SVG))),
            "brand/deepseek.svg" => return Ok(Some(Cow::Borrowed(BRAND_DEEPSEEK_SVG))),
            "brand/qwen.svg" => return Ok(Some(Cow::Borrowed(BRAND_QWEN_SVG))),
            "brand/zhipu.svg" => return Ok(Some(Cow::Borrowed(BRAND_ZHIPU_SVG))),
            "brand/grok.svg" => return Ok(Some(Cow::Borrowed(BRAND_GROK_SVG))),
            "brand/kimi.svg" => return Ok(Some(Cow::Borrowed(BRAND_KIMI_SVG))),
            "brand/minimax.svg" => return Ok(Some(Cow::Borrowed(BRAND_MINIMAX_SVG))),
            "brand/mistral.svg" => return Ok(Some(Cow::Borrowed(BRAND_MISTRAL_SVG))),
            "brand/meta.svg" => return Ok(Some(Cow::Borrowed(BRAND_META_SVG))),
            "brand/hunyuan.svg" => return Ok(Some(Cow::Borrowed(BRAND_HUNYUAN_SVG))),
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
            "brand/ai-mascot.png",
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

/// Tag surfaces and their matching text colors in picker order.
pub const TAG_COLORS: [(crate::i18n::BiText, u32, u32); 6] = [
    (crate::i18n::tags::TAG_COLOR_GRAY, 0xefeeeb, 0x56524b),
    (crate::i18n::tags::TAG_COLOR_BLUE, 0xe3edf8, 0x2d5377),
    (crate::i18n::tags::TAG_COLOR_GREEN, 0xe2f0e8, 0x2e5c46),
    (crate::i18n::tags::TAG_COLOR_ORANGE, 0xf5ead8, 0x704915),
    (crate::i18n::tags::TAG_COLOR_RED, 0xf5e4e4, 0x803f45),
    (crate::i18n::tags::TAG_COLOR_PURPLE, 0xebe5f5, 0x624785),
];

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
    if lower.ends_with(".drawio") || lower.ends_with(".drawio.svg") {
        return div()
            .flex_shrink_0()
            .text_color(rgb(0xd97706))
            .child(icon(IconName::LayoutGrid))
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
        "源码" | "所见即所得" | "Source" | "WYSIWYG" => (IconName::CodeXml, ""),
        "B" => (IconName::Bold, ""),
        "I" => (IconName::Italic, ""),
        "H" => (IconName::Heading2, ""),
        "•" => (IconName::List, ""),
        "☑" => (IconName::ListTodo, ""),
        "∑" => (IconName::Sigma, ""),
        "+  添加本地项目" | "+  Add Local Project" => (IconName::Plus, crate::i18n::workspace::ADD_LOCAL_PROJECT.get()),
        "‹" => (IconName::ChevronLeft, ""),
        "›" => (IconName::ChevronRight, ""),
        "↓" => (IconName::ArrowDown, ""),
        "✓" => (IconName::Check, ""),
        "📁+" => (IconName::FolderPlus, ""),
        "导出" | "Export" => (IconName::Download, ""),
        "⚙  设置" | "⚙  Settings" => (IconName::Settings, crate::i18n::workspace::SETTINGS_LABEL.get()),
        "终端" | "Terminal" => (IconName::SquareTerminal, ""),
        "▶  编译" | "▶  Compile" => (IconName::Play, ""),
        "■  停止" | "■  Stop" => (IconName::Square, ""),
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

/// Visual weight for an action inside [`ApprovalCard`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApprovalActionKind {
    Primary,
    Secondary,
    GhostDanger,
}

struct ApprovalAction {
    id: ElementId,
    label: SharedString,
    kind: ApprovalActionKind,
    on_click: Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>,
}

/// Reusable three-part confirmation card (header, scrollable body, tinted action footer)
/// with a subtle status accent strip for human-in-the-loop approvals across panes.
#[derive(IntoElement)]
pub struct ApprovalCard {
    id: ElementId,
    icon: IconName,
    title: SharedString,
    badge: (SharedString, u32, u32),
    accent_bar: u32,
    body: AnyElement,
    actions: Vec<ApprovalAction>,
    palette: crate::theme::Palette,
}

impl ApprovalCard {
    pub fn new(
        id: impl Into<ElementId>,
        title: impl Into<SharedString>,
        body: AnyElement,
        palette: crate::theme::Palette,
    ) -> Self {
        let dark = palette.is_dark();
        let (accent_bar, badge_bg, badge_fg) = if dark {
            (0xf59e0b, 0x3b2c12, 0xfcd34d)
        } else {
            (0xd97706, 0xfef3c7, 0x92400e)
        };
        Self {
            id: id.into(),
            icon: IconName::Shield,
            title: title.into(),
            badge: (crate::i18n::common::PENDING_APPROVAL.get().into(), badge_bg, badge_fg),
            accent_bar,
            body,
            actions: Vec::new(),
            palette,
        }
    }

    pub fn icon(mut self, icon: IconName) -> Self {
        self.icon = icon;
        self
    }

    #[allow(dead_code)]
    pub fn status_badge(
        mut self,
        label: impl Into<SharedString>,
        bg_hex: u32,
        fg_hex: u32,
    ) -> Self {
        self.badge = (label.into(), bg_hex, fg_hex);
        self
    }

    #[allow(dead_code)]
    pub fn accent_bar(mut self, hex: u32) -> Self {
        self.accent_bar = hex;
        self
    }

    pub fn action(
        mut self,
        id: impl Into<ElementId>,
        label: impl Into<SharedString>,
        kind: ApprovalActionKind,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.actions.push(ApprovalAction {
            id: id.into(),
            label: label.into(),
            kind,
            on_click: Box::new(on_click),
        });
        self
    }
}

impl RenderOnce for ApprovalCard {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let p = self.palette;
        let dark = p.is_dark();
        let (badge_text, _badge_bg, badge_fg) = self.badge;
        let has_actions = !self.actions.is_empty();
        let footer_actions = self.actions.into_iter().map(|act| {
            let el = match act.kind {
                ApprovalActionKind::GhostDanger => {
                    let hover_bg = if dark { 0x3b1d22 } else { 0xfff1f0 };
                    let hover_fg = if dark { 0xfca5a5 } else { 0xcf1322 };
                    button_base(act.id, act.label)
                        .min_h(px(26.))
                        .px_2p5()
                        .rounded(px(6.))
                        .text_size(px(12.))
                        .text_color(gpui_kit::rgb(p.muted))
                        .hover(move |s| {
                            s.bg(gpui_kit::rgb(hover_bg))
                                .text_color(gpui_kit::rgb(hover_fg))
                        })
                        .active(|s| s.shadow(inset_press(0.08)))
                }
                ApprovalActionKind::Secondary => button_base(act.id, act.label)
                    .min_h(px(26.))
                    .px_2p5()
                    .rounded(px(6.))
                    .text_size(px(12.))
                    .text_color(gpui_kit::rgb(p.text))
                    .bg(gpui_kit::rgb(p.hover))
                    .hover(move |s| s.bg(gpui_kit::rgb(p.pressed)))
                    .active(move |s| {
                        s.bg(gpui_kit::rgb(p.pressed))
                            .shadow(inset_press(0.08))
                    }),
                ApprovalActionKind::Primary => button_base(act.id, act.label)
                    .min_h(px(26.))
                    .px_3()
                    .rounded(px(6.))
                    .text_size(px(12.))
                    .font_medium()
                    .text_color(gpui_kit::rgb(p.on_accent))
                    .bg(gpui_kit::rgb(p.accent))
                    .shadow(raised(0.12, 0.12))
                    .hover(|s| s.opacity(0.90))
                    .active(|s| s.opacity(0.82).shadow(inset_press(0.18))),
            };
            el.on_click(move |ev, window, cx| (act.on_click)(ev, window, cx))
        });

        div()
            .id(self.id)
            .font_family(DEFAULT_SANS_FONT)
            .rounded(px(12.))
            .border_1()
            .border_color(gpui_kit::rgb(p.border))
            .bg(gpui_kit::rgb(p.surface))
            .shadow_sm()
            .p_3()
            .flex()
            .flex_col()
            .gap_2p5()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1p5()
                            .min_w_0()
                            .flex_1()
                            .child(
                                self::icon(self.icon)
                                    .size(px(14.))
                                    .text_color(gpui_kit::rgb(self.accent_bar))
                                    .flex_shrink_0(),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .text_ellipsis()
                                    .text_size(px(12.5))
                                    .font_medium()
                                    .text_color(gpui_kit::rgb(p.text))
                                    .child(self.title),
                            ),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .gap_1()
                            .child(
                                div()
                                    .w(px(6.))
                                    .h(px(6.))
                                    .rounded(px(3.))
                                    .bg(gpui_kit::rgb(self.accent_bar)),
                            )
                            .child(
                                div()
                                    .text_color(gpui_kit::rgb(badge_fg))
                                    .text_size(px(11.5))
                                    .font_medium()
                                    .child(badge_text),
                            ),
                    ),
            )
            .child(
                div()
                    .id("approval-card-body")
                    .max_h(px(176.))
                    .overflow_y_scroll()
                    .child(self.body),
            )
            .when(has_actions, |col| {
                col.child(
                    div()
                        .flex()
                        .items_center()
                        .justify_end()
                        .gap_1p5()
                        .children(footer_actions),
                )
            })
    }
}

/// Inset monospace command block for use inside [`ApprovalCard`].
pub fn approval_command_body(
    command: &str,
    font: SharedString,
    size: Pixels,
    palette: crate::theme::Palette,
) -> AnyElement {
    div()
        .w_full()
        .px_2p5()
        .py_2()
        .rounded(px(8.))
        .bg(gpui_kit::rgb(palette.code_background))
        .font_family(font)
        .text_size(size)
        .line_height(relative(1.45))
        .flex()
        .items_start()
        .gap_1p5()
        .child(
            div()
                .flex_shrink_0()
                .text_color(gpui_kit::rgb(palette.muted))
                .child("$"),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_color(gpui_kit::rgb(palette.text))
                .child(command.to_owned()),
        )
        .into_any_element()
}
