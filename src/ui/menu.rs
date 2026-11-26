//! Folio native context and dropdown menus.
//!
//! Replaces `gpui-component`'s default `PopupMenu` with a compact, paper-chrome
//! menu supporting Lucide icons, danger semantics, hairline dividers, section labels,
//! keyboard navigation, action dispatch with focus restoration, and edge-snapped anchoring.
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

use gpui_kit::{
    assets::IconName,
    base::{Selectable, StyledExt},
    component::{kbd::Kbd, popover::Popover},
    prelude::FluentBuilder,
    *,
};

const CONTEXT: &str = "FolioMenu";

gpui_kit::actions!(folio_menu, [SelectUp, SelectDown, Confirm, Dismiss]);

pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("up", SelectUp, Some(CONTEXT)),
        KeyBinding::new("down", SelectDown, Some(CONTEXT)),
        KeyBinding::new("enter", Confirm, Some(CONTEXT)),
        KeyBinding::new("escape", Dismiss, Some(CONTEXT)),
    ]);
}

pub enum FolioMenuItem {
    Separator,
    Label(SharedString),
    Action(FolioMenuEntry),
}

pub struct FolioMenuEntry {
    label: SharedString,
    icon: Option<IconName>,
    custom_icon: Option<Rc<dyn Fn(&mut Window, &mut App) -> AnyElement>>,
    shortcut: Option<SharedString>,
    checked: bool,
    disabled: bool,
    danger: bool,
    action: Option<Box<dyn Action>>,
    on_click: Option<Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>>,
}

impl FolioMenuItem {
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self::Action(FolioMenuEntry {
            label: label.into(),
            icon: None,
            custom_icon: None,
            shortcut: None,
            checked: false,
            disabled: false,
            danger: false,
            action: None,
            on_click: None,
        })
    }

    pub fn icon(mut self, icon: IconName) -> Self {
        if let Self::Action(entry) = &mut self {
            entry.icon = Some(icon);
        }
        self
    }

    pub fn custom_icon<E: IntoElement>(
        mut self,
        render: impl Fn(&mut Window, &mut App) -> E + 'static,
    ) -> Self {
        if let Self::Action(entry) = &mut self {
            entry.custom_icon = Some(Rc::new(move |w, cx| render(w, cx).into_any_element()));
        }
        self
    }

    pub fn shortcut(mut self, text: impl Into<SharedString>) -> Self {
        if let Self::Action(entry) = &mut self {
            entry.shortcut = Some(text.into());
        }
        self
    }

    pub fn checked(mut self, checked: bool) -> Self {
        if let Self::Action(entry) = &mut self {
            entry.checked = checked;
        }
        self
    }

    pub fn danger(mut self) -> Self {
        if let Self::Action(entry) = &mut self {
            entry.danger = true;
        }
        self
    }

    pub fn action(mut self, action: Box<dyn Action>) -> Self {
        if let Self::Action(entry) = &mut self {
            entry.action = Some(action);
        }
        self
    }

    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        if let Self::Action(entry) = &mut self {
            entry.on_click = Some(Rc::new(handler));
        }
        self
    }

    fn is_clickable(&self) -> bool {
        matches!(self, Self::Action(entry) if !entry.disabled)
    }

    fn has_icon(&self) -> bool {
        matches!(self, Self::Action(entry) if entry.icon.is_some() || entry.custom_icon.is_some())
    }
}

pub struct FolioMenu {
    focus_handle: FocusHandle,
    items: Vec<FolioMenuItem>,
    selected_index: Option<usize>,
    min_width: Pixels,
    max_height: Option<Pixels>,
    action_context: Option<FocusHandle>,
    trigger_focus: Option<FocusHandle>,
    previous_focus: Option<FocusHandle>,
    scroll_handle: ScrollHandle,
}

impl FluentBuilder for FolioMenu {}

impl EventEmitter<DismissEvent> for FolioMenu {}

impl Focusable for FolioMenu {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl FolioMenu {
    pub fn new(cx: &mut App) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            items: Vec::new(),
            selected_index: None,
            min_width: px(164.),
            max_height: None,
            action_context: None,
            trigger_focus: None,
            previous_focus: None,
            scroll_handle: ScrollHandle::new(),
        }
    }

    pub fn build(
        window: &mut Window,
        cx: &mut App,
        f: impl FnOnce(Self, &mut Window, &mut Context<Self>) -> Self,
    ) -> Entity<Self> {
        cx.new(|cx| f(Self::new(cx), window, cx))
    }

    pub fn min_w(mut self, width: Pixels) -> Self {
        self.min_width = width;
        self
    }

    pub fn item(mut self, item: FolioMenuItem) -> Self {
        self.items.push(item);
        self
    }

    pub fn separator(mut self) -> Self {
        if !self.items.is_empty() && !matches!(self.items.last(), Some(FolioMenuItem::Separator)) {
            self.items.push(FolioMenuItem::Separator);
        }
        self
    }

    pub fn label(mut self, text: impl Into<SharedString>) -> Self {
        self.items.push(FolioMenuItem::Label(text.into()));
        self
    }

    pub fn action_item(
        self,
        label: impl Into<SharedString>,
        icon: Option<IconName>,
        action: Box<dyn Action>,
    ) -> Self {
        let mut item = FolioMenuItem::new(label).action(action);
        if let Some(icon) = icon {
            item = item.icon(icon);
        }
        self.item(item)
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub(crate) fn set_focus_handles(
        &mut self,
        action_context: Option<FocusHandle>,
        trigger_focus: Option<FocusHandle>,
        previous_focus: Option<FocusHandle>,
    ) {
        self.action_context = action_context;
        self.trigger_focus = trigger_focus;
        self.previous_focus = previous_focus;
    }

    fn dismiss(&mut self, _: &Dismiss, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(previous) = self.previous_focus.take() {
            previous.focus(window, cx);
        }
        cx.emit(DismissEvent);
    }

    fn on_mouse_down_out(
        &mut self,
        _: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.dismiss(&Dismiss, window, cx);
    }

    fn select_up(&mut self, _: &SelectUp, _: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();
        let current = self.selected_index.unwrap_or(0);
        let prev = self
            .items
            .iter()
            .enumerate()
            .rev()
            .find(|(ix, item)| *ix < current && item.is_clickable())
            .or_else(|| {
                self.items
                    .iter()
                    .enumerate()
                    .rev()
                    .find(|(_, item)| item.is_clickable())
            })
            .map(|(ix, _)| ix);
        if self.selected_index != prev {
            self.selected_index = prev;
            cx.notify();
        }
    }

    fn select_down(&mut self, _: &SelectDown, _: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();
        let next = match self.selected_index {
            None => self
                .items
                .iter()
                .enumerate()
                .find(|(_, item)| item.is_clickable())
                .map(|(ix, _)| ix),
            Some(current) => self
                .items
                .iter()
                .enumerate()
                .find(|(ix, item)| *ix > current && item.is_clickable())
                .or_else(|| {
                    self.items
                        .iter()
                        .enumerate()
                        .find(|(_, item)| item.is_clickable())
                })
                .map(|(ix, _)| ix),
        };
        if self.selected_index != next {
            self.selected_index = next;
            cx.notify();
        }
    }

    fn confirm(&mut self, _: &Confirm, window: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();
        if let Some(ix) = self.selected_index {
            self.activate(ix, &ClickEvent::default(), window, cx);
        }
    }

    fn activate(&mut self, ix: usize, event: &ClickEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(FolioMenuItem::Action(entry)) = self.items.get(ix) else {
            return;
        };
        if entry.disabled {
            return;
        }
        let handler = entry.on_click.clone();
        let action = entry.action.as_ref().map(|a| a.boxed_clone());
        let restore_focus = self.action_context.clone().or_else(|| self.previous_focus.clone());

        if let Some(handler) = handler {
            self.previous_focus = None;
            handler(event, window, cx);
        } else if let Some(action) = action {
            self.previous_focus = None;
            if let Some(focus) = restore_focus {
                focus.focus(window, cx);
            }
            window.dispatch_action(action, cx);
        }
        cx.emit(DismissEvent);
    }
}

impl Render for FolioMenu {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = crate::theme::palette();
        let font_family = gpui_kit::component::Theme::global(cx).font_family.clone();
        let has_icons = self.items.iter().any(FolioMenuItem::has_icon);
        let item_count = self.items.len();
        let max_h = self
            .max_height
            .unwrap_or_else(|| (window.viewport_size().height * 0.55).min(px(420.)));
        let scrollable = item_count > 14 || self.max_height.is_some();

        let text_color = gpui_kit::rgb(palette.text);
        let muted_color = gpui_kit::rgb(palette.muted);
        let hover_bg = gpui_kit::rgb(palette.hover);
        let danger_fg = gpui_kit::rgb(palette.danger);
        let danger_bg = palette.danger_surface();
        let accent_color = gpui_kit::rgb(palette.accent);
        let divider_color = palette.divider();

        let rendered_items = self
            .items
            .iter()
            .enumerate()
            .filter(|(ix, item)| !(*ix + 1 == item_count && matches!(item, FolioMenuItem::Separator)))
            .map(|(ix, item)| match item {
                FolioMenuItem::Separator => div()
                    .w_full()
                    .h(px(1.))
                    .my(px(4.))
                    .bg(divider_color)
                    .into_any_element(),
                FolioMenuItem::Label(text) => div()
                    .px(px(8.))
                    .pt(px(5.))
                    .pb(px(2.))
                    .text_size(px(10.5))
                    .font_medium()
                    .text_color(muted_color)
                    .child(text.clone())
                    .into_any_element(),
                FolioMenuItem::Action(entry) => {
                    let selected = self.selected_index == Some(ix);
                    let disabled = entry.disabled;
                    let danger = entry.danger;
                    let checked = entry.checked;
                    let icon_el = if let Some(custom) = &entry.custom_icon {
                        Some(custom(window, cx))
                    } else if let Some(icon_name) = entry.icon {
                        Some(
                            super::icon(icon_name)
                                .size(px(13.5))
                                .text_color(if disabled {
                                    muted_color
                                } else if danger {
                                    danger_fg
                                } else if selected {
                                    text_color
                                } else {
                                    muted_color
                                })
                                .into_any_element(),
                        )
                    } else {
                        None
                    };
                    let binding_el = if let Some(shortcut) = &entry.shortcut {
                        Some(
                            div()
                                .ml_3()
                                .flex_shrink_0()
                                .text_size(px(11.))
                                .text_color(muted_color)
                                .child(shortcut.clone())
                                .into_any_element(),
                        )
                    } else if let Some(action) = &entry.action {
                        let kbd = self
                            .trigger_focus
                            .as_ref()
                            .and_then(|handle| {
                                Kbd::binding_for_action_in(action.as_ref(), handle, window)
                            })
                            .or_else(|| Kbd::binding_for_action(action.as_ref(), None, window));
                        kbd.map(|kbd| {
                            div()
                                .ml_3()
                                .flex_shrink_0()
                                .text_size(px(11.))
                                .text_color(muted_color)
                                .child(kbd.appearance(false))
                                .into_any_element()
                        })
                    } else {
                        None
                    };

                    div()
                        .id(("folio-menu-item", ix))
                        .h(px(26.))
                        .px(px(8.))
                        .rounded(px(6.))
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(8.))
                        .text_size(px(12.))
                        .text_color(if disabled {
                            muted_color
                        } else if danger {
                            danger_fg
                        } else {
                            text_color
                        })
                        .when(selected && !disabled, |row| {
                            row.bg(if danger { danger_bg } else { hover_bg })
                        })
                        .when(!disabled, |row| {
                            row.cursor_pointer()
                                .on_hover(cx.listener(move |menu, hovered, window, cx| {
                                    if *hovered {
                                        if menu.selected_index != Some(ix) {
                                            menu.selected_index = Some(ix);
                                            cx.notify();
                                        }
                                    } else if menu.selected_index == Some(ix)
                                        && !window.last_input_was_keyboard()
                                    {
                                        menu.selected_index = None;
                                        cx.notify();
                                    }
                                }))
                                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                .on_click(cx.listener(move |menu, event, window, cx| {
                                    cx.stop_propagation();
                                    window.prevent_default();
                                    menu.activate(ix, event, window, cx);
                                }))
                        })
                        .when(disabled, |row| row.opacity(0.5))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .items_center()
                                .gap(px(8.))
                                .when(has_icons, |left| {
                                    left.child(
                                        div()
                                            .w(px(14.))
                                            .h(px(14.))
                                            .flex_shrink_0()
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .children(icon_el),
                                    )
                                })
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .whitespace_nowrap()
                                        .text_ellipsis()
                                        .child(entry.label.clone()),
                                ),
                        )
                        .when(checked || binding_el.is_some(), |row| {
                            row.child(
                                div()
                                    .flex_shrink_0()
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .children(binding_el)
                                    .when(checked, |right| {
                                        right.child(
                                            super::icon(IconName::Check)
                                                .size(px(12.5))
                                                .text_color(accent_color),
                                        )
                                    }),
                            )
                        })
                        .into_any_element()
                }
            });

        let list = div()
            .id("folio-menu-items")
            .p(px(4.))
            .min_w(self.min_width)
            .max_w(px(340.))
            .flex()
            .flex_col()
            .gap(px(1.))
            .when(scrollable, |el| {
                el.max_h(max_h)
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll_handle)
            })
            .children(rendered_items);

        let card = div()
            .id("folio-menu")
            .key_context(CONTEXT)
            .track_focus(&self.focus_handle)
            .font_family(font_family)
            .rounded(px(10.))
            .bg(palette.elevated_surface())
            .border_1()
            .border_color(palette.card_border())
            .shadow(palette.popover_shadow())
            .text_color(text_color)
            .relative()
            .occlude()
            .on_action(cx.listener(Self::select_up))
            .on_action(cx.listener(Self::select_down))
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::dismiss))
            .on_mouse_down_out(cx.listener(Self::on_mouse_down_out))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(list)
            .when(scrollable, |el| {
                el.child(
                    div()
                        .absolute()
                        .top(px(4.))
                        .bottom(px(4.))
                        .right(px(1.))
                        .child(super::scrollbar_vertical(&self.scroll_handle)),
                )
            });

        if cx.reduce_motion() {
            card.into_any_element()
        } else {
            card.with_animation(
                "folio-menu-enter",
                Animation::new(std::time::Duration::from_millis(120)).with_easing(ease_out_quint()),
                |el, t| el.opacity(t),
            )
            .into_any_element()
        }
    }
}

// ---------------------------------------------------------------------------
// Context menu attachment
// ---------------------------------------------------------------------------

type MenuBuilder = dyn Fn(FolioMenu, &mut Window, &mut Context<FolioMenu>) -> FolioMenu;

pub trait FolioContextMenuExt: InteractiveElement + ParentElement + Styled {
    #[track_caller]
    fn folio_context_menu(
        mut self,
        f: impl Fn(FolioMenu, &mut Window, &mut Context<FolioMenu>) -> FolioMenu + 'static,
    ) -> FolioContextMenu<Self>
    where
        Self: Sized,
    {
        let caller = std::panic::Location::caller();
        let id = self
            .interactivity()
            .element_id
            .clone()
            .map(|id| ElementId::Name(format!("folio-context-menu-{id:?}").into()))
            .unwrap_or_else(|| ElementId::CodeLocation(*caller));
        FolioContextMenu {
            id,
            element: Some(self),
            menu: Some(Rc::new(f)),
            _ignore_style: StyleRefinement::default(),
        }
    }
}

impl<E: InteractiveElement + ParentElement + Styled> FolioContextMenuExt for E {}

pub struct FolioContextMenu<E: ParentElement + Styled + Sized> {
    id: ElementId,
    element: Option<E>,
    menu: Option<Rc<MenuBuilder>>,
    _ignore_style: StyleRefinement,
}

impl<E: ParentElement + Styled> ParentElement for FolioContextMenu<E> {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        if let Some(element) = &mut self.element {
            element.extend(elements);
        }
    }
}

impl<E: ParentElement + Styled> Styled for FolioContextMenu<E> {
    fn style(&mut self) -> &mut StyleRefinement {
        if let Some(element) = &mut self.element {
            element.style()
        } else {
            &mut self._ignore_style
        }
    }
}

impl<E: ParentElement + Styled + IntoElement + 'static> IntoElement for FolioContextMenu<E> {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

#[derive(Default)]
struct ContextMenuSharedState {
    menu_view: Option<Entity<FolioMenu>>,
    open: bool,
    position: Point<Pixels>,
    trigger_focus: Option<FocusHandle>,
    _subscription: Option<Subscription>,
}

pub struct FolioContextMenuState {
    element: Option<AnyElement>,
    draws_menu: Rc<Cell<bool>>,
    shared: Rc<RefCell<ContextMenuSharedState>>,
}

impl Default for FolioContextMenuState {
    fn default() -> Self {
        Self {
            element: None,
            draws_menu: Rc::default(),
            shared: Rc::new(RefCell::new(ContextMenuSharedState::default())),
        }
    }
}

struct DeferredFolioMenu {
    draws: Rc<Cell<bool>>,
    menu_view: Entity<FolioMenu>,
    position: Point<Pixels>,
    menu: Option<AnyElement>,
}

impl IntoElement for DeferredFolioMenu {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for DeferredFolioMenu {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let style = Style {
            position: Position::Absolute,
            ..Style::default()
        };
        (window.request_layout(style, None, cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        if !self.draws.get() {
            return;
        }
        let mut menu = deferred(
            anchored().child(
                div()
                    .w(window.bounds().size.width)
                    .h(window.bounds().size.height)
                    .on_scroll(|_, _, cx| cx.stop_propagation())
                    .child(
                        anchored()
                            .position(self.position)
                            .snap_to_window_with_margin(px(8.))
                            .anchor(Anchor::TopLeft)
                            .child(self.menu_view.clone()),
                    ),
            ),
        )
        .with_priority(gpui_kit::base::POPUP_PRIORITY)
        .into_any_element();
        menu.prepaint_as_root(bounds.origin, window.viewport_size().into(), window, cx);
        self.menu = Some(menu);
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        if let Some(menu) = &mut self.menu {
            menu.paint(window, cx);
        }
    }
}

impl<E: ParentElement + Styled + IntoElement + 'static> Element for FolioContextMenu<E> {
    type RequestLayoutState = FolioContextMenuState;
    type PrepaintState = Hitbox;

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        id: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        window.with_optional_element_state::<FolioContextMenuState, _>(id, |state, window| {
            let state = state.unwrap().unwrap_or_default();
            let (position, open, menu_view) = {
                let mut shared = state.shared.borrow_mut();
                shared.trigger_focus.get_or_insert_with(|| cx.focus_handle());
                (shared.position, shared.open, shared.menu_view.clone())
            };
            let draws_menu = Rc::new(Cell::new(false));
            let menu_element = menu_view
                .filter(|menu| open && !menu.read(cx).is_empty())
                .map(|menu_view| DeferredFolioMenu {
                    draws: draws_menu.clone(),
                    menu_view,
                    position,
                    menu: None,
                });
            let mut element = self
                .element
                .take()
                .expect("FolioContextMenu element exists")
                .children(menu_element)
                .into_any_element();
            let layout_id = element.request_layout(window, cx);
            let out = FolioContextMenuState {
                element: Some(element),
                draws_menu,
                shared: state.shared.clone(),
            };
            ((layout_id, out), Some(state))
        })
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        state: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        if let Some(trigger_focus) = state.shared.borrow().trigger_focus.as_ref() {
            window.set_focus_handle(trigger_focus, cx);
        }
        let position = state.shared.borrow().position;
        state.draws_menu.set(bounds.contains(&position));
        if let Some(element) = &mut state.element {
            element.prepaint(window, cx);
        }
        window.insert_hitbox(bounds, HitboxBehavior::Normal)
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        state: &mut Self::RequestLayoutState,
        hitbox: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let builder = self.menu.clone();
        let shared = state.shared.clone();
        let hitbox = hitbox.clone();
        window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
            if phase.bubble() && event.button == MouseButton::Right && hitbox.is_hovered(window) {
                cx.stop_propagation();
                open_context_menu(&shared, &builder, event.position, window, cx);
            }
        });
        if let Some(element) = &mut state.element {
            element.paint(window, cx);
        }
    }
}

fn open_context_menu(
    shared: &Rc<RefCell<ContextMenuSharedState>>,
    builder: &Option<Rc<MenuBuilder>>,
    position: Point<Pixels>,
    window: &mut Window,
    cx: &mut App,
) {
    let Some(builder) = builder else {
        return;
    };
    let previous_focus = window.focused(cx).and_then(|focused| {
        let state = shared.borrow();
        match state.menu_view.as_ref() {
            Some(menu) if menu.read(cx).focus_handle == focused => {
                menu.read(cx).previous_focus.clone()
            }
            _ => Some(focused),
        }
    });
    let trigger_focus = shared.borrow().trigger_focus.clone();
    let builder = builder.clone();
    let menu = FolioMenu::build(window, cx, move |menu, window, cx| builder(menu, window, cx));
    if menu.read(cx).is_empty() {
        return;
    }
    menu.update(cx, |menu, _| {
        menu.set_focus_handles(previous_focus.clone(), trigger_focus, previous_focus);
    });
    menu.focus_handle(cx).focus(window, cx);

    let shared_dismiss = shared.clone();
    let subscription = window.subscribe(&menu, cx, move |_, _: &DismissEvent, window, _| {
        let mut state = shared_dismiss.borrow_mut();
        state.open = false;
        state.menu_view = None;
        state._subscription = None;
        window.refresh();
    });

    {
        let mut state = shared.borrow_mut();
        state.position = position;
        state.open = true;
        state.menu_view = Some(menu);
        state._subscription = Some(subscription);
    }
    window.refresh();
}

// ---------------------------------------------------------------------------
// Dropdown menu attachment (reuses Popover positioning engine with appearance(false))
// ---------------------------------------------------------------------------

pub trait FolioDropdownMenuExt:
    Styled + Selectable + InteractiveElement + IntoElement + 'static
{
    fn folio_dropdown_menu(
        self,
        f: impl Fn(FolioMenu, &mut Window, &mut Context<FolioMenu>) -> FolioMenu + 'static,
    ) -> FolioDropdownMenu<Self> {
        self.folio_dropdown_menu_with_anchor(Anchor::TopLeft, f)
    }

    fn folio_dropdown_menu_with_anchor(
        mut self,
        anchor: impl Into<Anchor>,
        f: impl Fn(FolioMenu, &mut Window, &mut Context<FolioMenu>) -> FolioMenu + 'static,
    ) -> FolioDropdownMenu<Self> {
        let id = self.interactivity().element_id.clone().unwrap_or(0.into());
        FolioDropdownMenu {
            id: SharedString::from(format!("folio-dropdown:{id:?}")).into(),
            anchor: anchor.into(),
            trigger: self,
            builder: Rc::new(f),
        }
    }
}

impl<T: Styled + Selectable + InteractiveElement + IntoElement + 'static> FolioDropdownMenuExt for T {}

#[derive(IntoElement)]
pub struct FolioDropdownMenu<T: Selectable + IntoElement + 'static> {
    id: ElementId,
    anchor: Anchor,
    trigger: T,
    builder: Rc<MenuBuilder>,
}

#[derive(Default)]
struct DropdownState {
    menu: Option<Entity<FolioMenu>>,
}

impl<T: Selectable + IntoElement + 'static> RenderOnce for FolioDropdownMenu<T> {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let builder = self.builder.clone();
        let menu_state =
            window.use_keyed_state(self.id.clone(), cx, |_, _| DropdownState::default());

        Popover::new(SharedString::from(format!("folio-popover:{}", self.id)))
            .appearance(false)
            .overlay_closable(false)
            .trigger(self.trigger)
            .anchor(self.anchor)
            .content(move |_, window, cx| {
                let menu = match menu_state.read(cx).menu.clone() {
                    Some(menu) => menu,
                    None => {
                        let builder = builder.clone();
                        let previous_focus = window.focused(cx);
                        let menu = FolioMenu::build(window, cx, move |menu, window, cx| {
                            builder(menu, window, cx)
                        });
                        menu.update(cx, |menu, _| {
                            menu.set_focus_handles(previous_focus.clone(), None, previous_focus);
                        });
                        menu_state.update(cx, |state, _| {
                            state.menu = Some(menu.clone());
                        });
                        menu.focus_handle(cx).focus(window, cx);

                        let popover_state = cx.entity().downgrade();
                        let menu_state_weak = menu_state.downgrade();
                        window
                            .subscribe(&menu, cx, move |_, _: &DismissEvent, window, cx| {
                                if let Some(popover) = popover_state.upgrade() {
                                    popover.update(cx, |state, cx| state.dismiss(window, cx));
                                }
                                let _ = menu_state_weak.update(cx, |state, _| state.menu = None);
                            })
                            .detach();
                        menu
                    }
                };
                div().child(menu)
            })
    }
}
