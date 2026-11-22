# Folio 原生排版与 PDF 导出

- `latex-rust 3.0.3`：增加宿主字体的字形度量回调，并在数学盒模型保留字形所属字体。回退字形不使用主数学字体的脚本替换与重音度量。Folio 只消费盒模型，不调用该库的图片输出后端。
- `gpui-pre 0.3.12`：增加文档绘制捕获，保留字形、原始文字簇、字体、图形、图片及裁剪；捕获期间不创建 GPU 字形纹理。独立文档布局可以覆盖视口裁剪，以包含未滚动到的内容。
- `gpui-pre-windows 0.3.12`：从 DirectWrite 取得实际字体文件和集合索引，供 PDF 字体嵌入及文字复制使用。
- `gpui-pre-windows 0.3.12`：读取文件剪贴板时将 `CF_HDROP` 句柄直接传给 `DragQueryFileW`，供资源管理器文件粘贴使用。
- `gpui-kit-markdown-editor`：增加使用同一快照、节点渲染器的完整文档视图，省略编辑选区、光标和视口裁剪，并记录分页使用的块边界。
- `gpui-pre 0.3.12`：增加 `VectorPaint::Svg` 与 `Window::paint_svg_document`，把图表 SVG 原文连同裁剪区域记录进文档捕获，供 PDF 用 `krilla-svg` 直接写成矢量图形与可选文字；不截图、不引入第二套排版。
- `gpui-kit-markdown-editor`：可选单语言代码块预览接口只改变视图，保留原代码模型与序列化；源码模式复用原编辑几何，预览保留真实源码长度，完整文档导出强制预览且不带工具栏。
- `gpui-kit-markdown-editor`：按文档 revision 缓存标题块序列，大纲跳转按标题索引定位，活动标题同步只遍历标题块。
- `gpui-kit-markdown-editor`：批注的选区计数和跳转共用引用匹配，跨块引用按已保存的 Markdown 摘录匹配连续叶块。保留 inline atom 的源范围映射，并用唯一的正文高亮恢复位次已变化的批注。
- `gpui-kit-markdown-editor`：公式规范化遇到没有 TeX 起始定界符的文本时跳过预解析，减少 AI 流式渲染和 Markdown 导入的重复解析。
- `gpui-base 0.9.1`：为 Folio 的 Markdown 源码编辑器提供后台全文换行。宽度变化时保留现有索引，按实际字体与编辑区几何异步重排。长逻辑行的字形只按视口附近的软换行行整形，原文偏移和折行行号映射保留完整。
- `gpui-base` 与 `gpui-kit-markdown-editor`：编辑区和 PDF 共享列表分组与相邻容器的留白。透明编辑区同时淡化行内代码的胶囊底色和标记背景。
- `gpui-base 0.9.1`：在 vendored crate 的 Cargo 清单中关闭该 crate 的编译警告。

Mermaid 适配保留编辑器原有的 `code_ui` 模块、投影缓存和折叠按钮 bounds；自定义节点使用 `editor_host::BlockState`。预览点击在编辑器 entity 的 `update` 内设置焦点，避免通过 `read(cx)` 跨越可变 App 借用。

字体沿用 `resources/fonts`，中文公式缺字时才读取系统微软雅黑。Markdown PDF 使用 `krilla` 写入上述绘制操作；整篇 `.tex` 编译保持 `latexmk` 流程。
- `gpui-pdf`：新增 `PdfView::flash_to_page`，用于从知识库或外部定位跳转至目标页顶部并触发 1.4 秒临时高亮边框（冷打开时由 `pending_reveal` 复用该闪烁高亮）。
