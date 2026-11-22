//! Window-modal confirmations and the response consumed when an action resumes.

use super::Workspace;
use gpui_kit::{AnyElement, Context, IntoElement, ParentElement, StatefulInteractiveElement, Styled, Window, div, px};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Dialog {
    Drafts,
    CloseTab,
    DeleteSelected,
    RemoveProject,
    TrashPurge,
    TrashClear,
    TrashConflict,
    DeleteTag,
}

pub(super) enum DialogButtons {
    SaveDiscardCancel,
    Confirm(String),
    RestoreConflict,
}

struct DialogPrompt {
    kind: Dialog,
    title: String,
    description: String,
    buttons: DialogButtons,
    retry: Box<dyn FnOnce(&mut Workspace, &mut Window, &mut Context<Workspace>)>,
}

#[derive(Default)]
pub(super) struct Dialogs {
    prompt: Option<DialogPrompt>,
    response: Option<(Dialog, rfd::MessageDialogResult)>,
    open: bool,
}

impl Dialogs {
    pub(super) fn has_response(&self, kind: Dialog) -> bool {
        self.response.as_ref().is_some_and(|(pending, _)| *pending == kind)
    }

    pub(super) fn is_open(&self) -> bool {
        self.open
    }

    pub(super) fn blocks_workspace(&self) -> bool {
        self.open || self.prompt.is_some()
    }

    pub(super) fn set_native_open(&mut self, open: bool) {
        self.open = open;
    }
}

impl Workspace {
    /// Re-enter the original action after a choice, then consume its response once.
    pub(super) fn ask(
        &mut self,
        kind: Dialog,
        title: String,
        description: String,
        buttons: DialogButtons,
        cx: &mut Context<Self>,
        retry: impl FnOnce(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> Option<rfd::MessageDialogResult> {
        if self.dialogs.has_response(kind) {
            return Some(self.dialogs.response.take().expect("matched dialog response").1);
        }
        self.dialogs.open = true;
        self.dialogs.prompt = Some(DialogPrompt {
            kind,
            title,
            description,
            buttons,
            retry: Box::new(retry),
        });
        cx.notify();
        None
    }

    fn answer_dialog(
        &mut self,
        result: rfd::MessageDialogResult,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let prompt = self.dialogs.prompt.take().expect("visible prompt exists");
        self.dialogs.open = false;
        self.dialogs.response = Some((prompt.kind, result));
        (prompt.retry)(self, window, cx);
        cx.notify();
    }

    pub(super) fn render_dialog(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let prompt = self.dialogs.prompt.as_ref()?;
        // Confirm dialogs are only used for destructive actions.
        let destructive = matches!(prompt.buttons, DialogButtons::Confirm(_));
        let choices = match &prompt.buttons {
            DialogButtons::SaveDiscardCancel => vec![
                (
                    crate::i18n::workspace::BTN_KEEP_EDITING.get().to_string(),
                    rfd::MessageDialogResult::Cancel,
                    false,
                ),
                (
                    crate::i18n::workspace::BTN_DISCARD_CHANGES.get().to_string(),
                    rfd::MessageDialogResult::No,
                    false,
                ),
                (
                    crate::i18n::common::SAVE.get().to_string(),
                    rfd::MessageDialogResult::Yes,
                    true,
                ),
            ],
            DialogButtons::Confirm(confirm) => vec![
                (
                    crate::i18n::common::CANCEL.get().to_string(),
                    rfd::MessageDialogResult::Cancel,
                    false,
                ),
                (confirm.clone(), rfd::MessageDialogResult::Ok, true),
            ],
            DialogButtons::RestoreConflict => vec![
                (
                    crate::i18n::common::SKIP.get().to_string(),
                    rfd::MessageDialogResult::Cancel,
                    false,
                ),
                (
                    crate::i18n::common::KEEP_BOTH.get().to_string(),
                    rfd::MessageDialogResult::No,
                    false,
                ),
                (
                    crate::i18n::common::REPLACE.get().to_string(),
                    rfd::MessageDialogResult::Yes,
                    true,
                ),
            ],
        };
        let choices =
            choices
                .into_iter()
                .enumerate()
                .map(|(index, (label, result, primary))| {
                    let id = ("prompt-choice", index);
                    let button = match (primary, destructive) {
                        (true, true) => crate::ui::button_danger(id, label),
                        (true, false) => crate::ui::button_primary(id, label),
                        (false, _) => crate::ui::button_secondary(id, label).px_3(),
                    }
                    .min_w(px(64.))
                    .on_click(cx.listener(
                        move |app, _, window, cx| app.answer_dialog(result.clone(), window, cx),
                    ));
                    button.into_any_element()
                });
        Some(crate::ui::modal_overlay(
            prompt.title.clone(),
            prompt.description.clone(),
            div().flex().justify_end().gap_2().children(choices),
        ).into_any_element())
    }
}
