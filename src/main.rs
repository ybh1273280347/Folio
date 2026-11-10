//! Native editor rendering pilot for Folio's potential GPUI migration.

use gpui_kit::component::input::{Editor, EditorState};
use gpui_kit::*;

struct Prototype {
    editors: [Entity<EditorState>; 3],
}

impl Render for Prototype {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(0xf4f3f0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px_4()
                    .py_3()
                    .bg(rgb(0xffffff))
                    .border_b_1()
                    .border_color(rgb(0xe3e3e0))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(div().text_sm().font_semibold().text_color(rgb(0x34383d)).child("Folio / GPUI"))
                            .child(div().text_xs().text_color(rgb(0x777c82)).child("Native editor pilot")),
                    )
                    .child(div().text_xs().text_color(rgb(0x777c82)).child("3 documents open")),
            )
            .child(
                div()
                    .flex()
                    .flex_1()
                    .gap_2()
                    .p_3()
                    .children(self.editors.iter().enumerate().map(|(index, state)| {
                        let (name, language) = match index {
                            0 => ("main.tex", "\\documentclass{article}"),
                            1 => ("methods.tex", "\\section{Methods}"),
                            _ => ("results.tex", "\\section{Results}"),
                        };

                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .rounded_md()
                            .border_1()
                            .border_color(rgb(0xe3e3e0))
                            .bg(rgb(0xffffff))
                            .child(
                                div()
                                    .px_3()
                                    .py_2()
                                    .border_b_1()
                                    .border_color(rgb(0xe9e9e6))
                                    .text_xs()
                                    .text_color(rgb(0x555a60))
                                    .child(name),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_h_0()
                                    .p_2()
                                    .child(
                                        Editor::new(state)
                                            .appearance(false)
                                            .bordered(false)
                                            .size_full(),
                                    ),
                            )
                            .child(div().px_3().py_2().text_xs().text_color(rgb(0x8a8f95)).child(language))
                    })),
            )
            .child(
                div()
                    .px_4()
                    .py_2()
                    .bg(rgb(0xffffff))
                    .border_t_1()
                    .border_color(rgb(0xe3e3e0))
                    .text_xs()
                    .text_color(rgb(0x777c82))
                    .child("Three independent native editor states"),
            )
    }
}

fn main() {
    gpui_kit::application().run(|cx| {
        gpui_kit::init(cx);
        gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
            // Keep three independent buffers alive to measure concurrent editor cost.
            let editors = [
                "\\documentclass{article}\n\\usepackage{amsmath}\n\\begin{document}\n\\title{Least Squares}\n\\maketitle\n\\section{Introduction}\nA compact LaTeX source editor pilot.\n\\end{document}\n",
                "\\section{Methods}\nLet $X$ be the design matrix and $y$ the observations.\n\\begin{equation}\n\\hat{\\beta} = (X^T X)^{-1}X^T y\n\\end{equation}\n",
                "\\section{Results}\n\\begin{table}\n\\centering\n\\begin{tabular}{lr}\nModel & RMSE \\\\\n\\hline\nOLS & 0.42 \\\\\n\\end{tabular}\n\\end{table}\n",
            ]
            .map(|text| cx.new(|cx| EditorState::new(window, cx).language("latex").default_value(text)));
            cx.new(|_| Prototype { editors })
        })
        .expect("failed to open GPUI prototype window");
    });
}
