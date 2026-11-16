use std::{
    cell::RefCell,
    collections::HashMap,
    rc::Rc,
    sync::Arc,
};

use gpui_kit::{
    base::text::editor_host::CodeBlock,
    *,
};
use markdown_editor::BlockId;
use rusty_mermaid::{
    Color as MermaidColor,
    Scene,
    Theme as MermaidTheme,
};
use rusty_mermaid_gpui::{
    PreparedScene,
    render_element,
};

#[derive(Clone, Copy, PartialEq)]
struct Appearance {
    palette: crate::theme::Palette,
    font_scale: f32,
}

enum Status {
    Pending,
    Empty,
    Ready {
        prepared: Rc<RefCell<PreparedScene>>,
    },
    Failed(String),
}

impl Status {
    fn for_source(source: &str) -> Self {
        if source.trim().is_empty() { Self::Empty } else { Self::Pending }
    }
}

struct BlockState {
    revision: u64,
    appearance: Appearance,
    job: Option<u64>,
    status: Status,
}

impl BlockState {
    fn pending(revision: u64, appearance: Appearance, job: u64) -> Self {
        Self {
            revision,
            appearance,
            job: Some(job),
            status: Status::Pending,
        }
    }

    fn matches(&self, revision: u64, appearance: Appearance, job: u64) -> bool {
        self.revision == revision
            && self.appearance == appearance
            && self.job == Some(job)
    }

    fn accept_completion(
        &mut self,
        revision: u64,
        appearance: Appearance,
        job: u64,
        status: Status,
    ) -> bool {
        if !self.matches(revision, appearance, job) {
            return false;
        }
        self.status = status;
        self.job = None;
        true
    }
}

pub struct MermaidReady;

impl EventEmitter<MermaidReady> for MermaidRenderer {}

pub struct MermaidRenderer {
    blocks: HashMap<BlockId, BlockState>,
    jobs: HashMap<BlockId, (u64, Task<()>)>,
    next_job: u64,
}

impl MermaidRenderer {
    pub fn new(_cx: &mut Context<Self>) -> Self {
        Self {
            blocks: HashMap::new(),
            jobs: HashMap::new(),
            next_job: 0,
        }
    }

    pub fn element(
        &mut self,
        id: BlockId,
        revision: u64,
        code: &CodeBlock,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let appearance = appearance(window);
        if let Some(state) = self.blocks.get_mut(&id) {
            if state.revision == revision && state.appearance == appearance {
                let error = match &state.status {
                    Status::Ready { prepared } => prepared.borrow().paint_error(),
                    _ => None,
                };
                if let Some(error) = error {
                    state.status = Status::Failed(error);
                }
                return status_element(&state.status, appearance);
            }
        }

        self.jobs.remove(&id);
        self.next_job = self.next_job.wrapping_add(1);
        let job = self.next_job;
        let source = code.code().to_string();
        if matches!(Status::for_source(&source), Status::Empty) {
            self.blocks.insert(
                id,
                BlockState {
                    revision,
                    appearance,
                    job: None,
                    status: Status::Empty,
                },
            );
            return status_element(&Status::Empty, appearance);
        }

        let theme = mermaid_theme(appearance);
        self.blocks.insert(id, BlockState::pending(revision, appearance, job));
        let task = cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    rusty_mermaid::render(&source, &theme).map_err(|error| error.to_string())
                })
                .await;
            let _ = this.update(cx, |renderer, cx| {
                renderer.finish(id, revision, appearance, job, result, cx);
            });
        });
        self.jobs.insert(id, (job, task));
        status_element(&Status::Pending, appearance)
    }

    pub fn retain_blocks(&mut self, ids: impl IntoIterator<Item = BlockId>) {
        let retained: std::collections::HashSet<_> = ids.into_iter().collect();
        self.blocks.retain(|id, _| retained.contains(id));
        self.jobs.retain(|id, _| retained.contains(id));
    }

    pub fn is_idle(&self) -> bool {
        self.jobs.is_empty()
    }

    fn finish(
        &mut self,
        id: BlockId,
        revision: u64,
        appearance: Appearance,
        job: u64,
        result: Result<Scene, String>,
        cx: &mut Context<Self>,
    ) {
        let Some(state) = self.blocks.get(&id) else {
            return;
        };
        if !state.matches(revision, appearance, job) {
            return;
        }

        let status = match result {
            Err(error) => Status::Failed(error),
            Ok(scene) => match PreparedScene::new(Arc::new(scene), mermaid_theme(appearance)) {
                Ok(prepared) => Status::Ready {
                    prepared: Rc::new(RefCell::new(prepared)),
                },
                Err(error) => Status::Failed(error),
            },
        };

        let accepted = self
            .blocks
            .get_mut(&id)
            .expect("matching Mermaid block")
            .accept_completion(revision, appearance, job, status);
        if !accepted {
            return;
        }
        if self.jobs.get(&id).is_some_and(|(current, _)| *current == job) {
            self.jobs.remove(&id);
        }
        cx.emit(MermaidReady);
        cx.notify();
    }
}

fn appearance(window: &Window) -> Appearance {
    let palette = crate::theme::palette();
    let font_size = window
        .text_style()
        .font_size
        .to_pixels(window.rem_size());
    Appearance {
        palette,
        font_scale: f32::from(font_size) / 14.0,
    }
}

fn mermaid_color(value: u32) -> MermaidColor {
    MermaidColor::rgb(
        ((value >> 16) & 0xff) as u8,
        ((value >> 8) & 0xff) as u8,
        (value & 0xff) as u8,
    )
}

fn mermaid_theme(appearance: Appearance) -> MermaidTheme {
    let mut theme = if appearance.palette.is_dark() {
        MermaidTheme::dark()
    } else {
        MermaidTheme::light()
    };
    theme.background = mermaid_color(appearance.palette.surface);
    theme.node_text = mermaid_color(appearance.palette.text);
    theme.edge_label_text = mermaid_color(appearance.palette.text);
    theme.edge_stroke = mermaid_color(appearance.palette.text);
    theme.muted_text = mermaid_color(appearance.palette.muted);
    theme
}

fn status_element(status: &Status, appearance: Appearance) -> AnyElement {
    match status {
        Status::Ready { prepared, .. } => {
            render_element(Rc::clone(prepared), appearance.font_scale).into_any_element()
        }
        Status::Empty => div()
            .text_color(crate::theme::color(appearance.palette.muted))
            .child("Mermaid 图表为空")
            .into_any_element(),
        Status::Pending => div()
            .text_color(crate::theme::color(appearance.palette.muted))
            .child("图表排版中…")
            .into_any_element(),
        Status::Failed(error) => div()
            .text_color(crate::theme::color(appearance.palette.danger))
            .child(format!("Mermaid 渲染失败：{error}"))
            .into_any_element(),
    }
}

#[cfg(test)]
mod tests {
    use super::{Appearance, BlockState, Status};
    use rusty_mermaid::{BBox, Point, Primitive, Scene, Theme, render};
    use rusty_mermaid_core::{EdgeBinding, ElementId, MarkerType, Transform};
    use rusty_mermaid_gpui::validate_scene;

    const UNSUPPORTED_TRANSFORM_ERROR: &str = "当前原生绘制后端不支持此图表变换";

    fn render_scene(input: &str) -> Scene {
        render(input, &Theme::default()).expect("Mermaid input should render")
    }

    fn text_labels(scene: &Scene) -> impl Iterator<Item = &str> {
        scene.elements().iter().filter_map(|element| {
            if let Primitive::Text { content, .. } = &element.primitive {
                Some(content.as_str())
            } else {
                None
            }
        })
    }

    fn has_text(scene: &Scene, expected: &str) -> bool {
        text_labels(scene).any(|label| label == expected)
    }

    fn count_primitives(scene: &Scene, predicate: impl Fn(&Primitive) -> bool) -> usize {
        scene
            .elements()
            .iter()
            .filter(|element| predicate(&element.primitive))
            .count()
    }

    fn assert_valid_scene(scene: &Scene) {
        assert_eq!(validate_scene(scene), Ok(()));
    }

    #[test]
    fn flowchart_preserves_labels_shapes_and_edge_binding() {
        let scene = render_scene("flowchart TD\nA[开始] --> B[结束]");

        assert!(has_text(&scene, "开始"));
        assert!(has_text(&scene, "结束"));
        assert!(count_primitives(&scene, |primitive| {
            matches!(primitive, Primitive::Rect { .. })
        }) >= 2);
        assert!(count_primitives(&scene, |primitive| {
            matches!(
                primitive,
                Primitive::Path {
                    marker_end: Some(MarkerType::ArrowPoint),
                    ..
                }
            )
        }) >= 1);
        assert_eq!(
            scene.edge_bindings(),
            &[EdgeBinding {
                edge: ElementId::edge("A->B#0"),
                src: ElementId::node("A"),
                dst: ElementId::node("B"),
            }]
        );
        assert_valid_scene(&scene);
    }

    #[test]
    fn sequence_preserves_participants_message_and_edge_binding() {
        let scene = render_scene("sequenceDiagram\nAlice->>Bob: 你好");

        assert!(has_text(&scene, "Alice"));
        assert!(has_text(&scene, "Bob"));
        assert!(has_text(&scene, "你好"));
        assert!(count_primitives(&scene, |primitive| {
            matches!(primitive, Primitive::Circle { .. } | Primitive::Rect { .. })
        }) >= 2);
        assert!(count_primitives(&scene, |primitive| {
            matches!(
                primitive,
                Primitive::Path {
                    marker_end: Some(MarkerType::ArrowPoint),
                    ..
                }
            )
        }) >= 1);
        assert_eq!(scene.edge_bindings().len(), 1);
        assert_eq!(scene.edge_bindings()[0].src, ElementId::node("Alice"));
        assert_eq!(scene.edge_bindings()[0].dst, ElementId::node("Bob"));
        assert_valid_scene(&scene);
    }

    #[test]
    fn state_preserves_ready_state_start_shape_and_transition() {
        let scene = render_scene("stateDiagram-v2\n[*] --> Ready");

        assert!(has_text(&scene, "Ready"));
        assert!(count_primitives(&scene, |primitive| {
            matches!(primitive, Primitive::Circle { .. })
        }) >= 1);
        assert!(count_primitives(&scene, |primitive| {
            matches!(primitive, Primitive::Rect { .. })
        }) >= 1);
        assert!(count_primitives(&scene, |primitive| {
            matches!(
                primitive,
                Primitive::Path {
                    marker_end: Some(MarkerType::ArrowPoint),
                    ..
                }
            )
        }) >= 1);
        assert_eq!(scene.edge_bindings().len(), 1);
        assert_eq!(scene.edge_bindings()[0].dst, ElementId::node("Ready"));
    }

    #[test]
    fn class_preserves_class_shapes_labels_and_inheritance_edge() {
        let scene = render_scene("classDiagram\nAnimal <|-- Duck");

        assert!(has_text(&scene, "Animal"));
        assert!(has_text(&scene, "Duck"));
        assert!(count_primitives(&scene, |primitive| {
            matches!(primitive, Primitive::Rect { .. })
        }) >= 2);
        assert!(count_primitives(&scene, |primitive| {
            matches!(
                primitive,
                Primitive::Path {
                    marker_start: Some(MarkerType::ArrowBarb),
                    ..
                }
            )
        }) >= 1);
        assert_eq!(scene.edge_bindings().len(), 1);
        assert_eq!(scene.edge_bindings()[0].src, ElementId::node("Animal"));
        assert_eq!(scene.edge_bindings()[0].dst, ElementId::node("Duck"));
        assert_valid_scene(&scene);
    }

    #[test]
    fn pie_preserves_slice_labels_and_arcs() {
        let scene = render_scene("pie\n\"完成\" : 8\n\"剩余\" : 2");

        assert!(has_text(&scene, "完成"));
        assert!(has_text(&scene, "剩余"));
        assert_eq!(
            count_primitives(&scene, |primitive| {
                matches!(primitive, Primitive::Arc { .. })
            }),
            2
        );
        assert_valid_scene(&scene);
    }

    #[test]
    fn parser_rejects_empty_and_non_diagram_input() {
        let theme = Theme::default();

        assert!(render("", &theme).is_err());
        assert!(render("not-a-diagram", &theme).is_err());
    }

    #[test]
    fn gpui_rejects_rotation_and_nonuniform_scale() {
        let mut rotated = Scene::new(100.0, 100.0);
        rotated.push(Primitive::Group {
            transform: Transform::Rotate {
                degrees: 15.0,
                cx: 50.0,
                cy: 50.0,
            },
            children: vec![Primitive::Rect {
                bbox: BBox::new(50.0, 50.0, 20.0, 20.0),
                rx: 0.0,
                ry: 0.0,
                style: Default::default(),
            }],
        });
        assert_eq!(
            validate_scene(&rotated),
            Err(UNSUPPORTED_TRANSFORM_ERROR.to_owned())
        );

        let mut nonuniform = Scene::new(100.0, 100.0);
        nonuniform.push(Primitive::Group {
            transform: Transform::Scale(2.0, 3.0),
            children: vec![Primitive::Text {
                position: Point::new(10.0, 10.0),
                content: "text".into(),
                anchor: Default::default(),
                style: Default::default(),
            }],
        });
        assert_eq!(
            validate_scene(&nonuniform),
            Err(UNSUPPORTED_TRANSFORM_ERROR.to_owned())
        );
    }

    fn test_appearance(text: u32) -> Appearance {
        let mut palette = crate::theme::Palette::light();
        palette.text = text;
        Appearance {
            palette,
            font_scale: 1.0,
        }
    }

    #[test]
    fn stale_request_a_does_not_replace_request_b() {
        let appearance = test_appearance(0x111111);
        let mut state = BlockState::pending(2, appearance, 2);

        assert!(!state.accept_completion(
            1,
            appearance,
            1,
            Status::Failed("stale".into())
        ));
        assert!(matches!(state.status, Status::Pending));
        assert_eq!(state.job, Some(2));
        assert!(state.accept_completion(2, appearance, 2, Status::Failed("current".into())));
        assert!(matches!(&state.status, Status::Failed(error) if error == "current"));
        assert_eq!(state.job, None);
    }

    #[test]
    fn appearance_switch_rejects_old_completion() {
        let old = test_appearance(0x111111);
        let new = Appearance { palette: crate::theme::Palette::dark(), ..old };
        let mut state = BlockState::pending(7, new, 2);

        assert!(!state.accept_completion(7, old, 1, Status::Failed("stale".into())));
        assert!(matches!(state.status, Status::Pending));
        assert_eq!(state.job, Some(2));
        assert!(state.accept_completion(7, new, 2, Status::Failed("dark".into())));
        assert_eq!(state.appearance.palette, crate::theme::Palette::dark());
    }

    #[test]
    fn empty_state_stays_distinct_from_render_error() {
        let appearance = test_appearance(0x111111);
        assert!(matches!(Status::for_source(" \n\t"), Status::Empty));
        assert!(matches!(Status::for_source("not-a-diagram"), Status::Pending));
        let mut error_state = BlockState::pending(4, appearance, 1);
        assert!(error_state.accept_completion(
            4,
            appearance,
            1,
            Status::Failed(render("not-a-diagram", &Theme::default()).unwrap_err().to_string())
        ));
        assert!(matches!(&error_state.status, Status::Failed(error) if !error.is_empty()));
    }
}
