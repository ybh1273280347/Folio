# Folio 原生排版与 PDF 导出

- `latex-rust 3.0.3`：增加宿主字体的字形度量回调，并在数学盒模型保留字形所属字体。回退字形不使用主数学字体的脚本替换与重音度量。Folio 只消费盒模型，不调用该库的图片输出后端。
- `gpui-pre 0.3.12`：增加文档绘制捕获，保留字形、原始文字簇、字体、图形、图片及裁剪；捕获期间不创建 GPU 字形纹理。独立文档布局可以覆盖视口裁剪，以包含未滚动到的内容。
- `gpui-pre-windows 0.3.12`：从 DirectWrite 取得实际字体文件和集合索引，供 PDF 字体嵌入及文字复制使用。
- `gpui-kit-markdown-editor`：增加使用同一快照、节点渲染器的完整文档视图，省略编辑选区、光标和视口裁剪，并记录分页使用的块边界。
- `gpui-pre 0.3.12`：增加 `VectorPaint::Svg` 与 `Window::paint_svg_document`，把图表 SVG 原文连同裁剪区域记录进文档捕获，供 PDF 用 `krilla-svg` 直接写成矢量图形与可选文字；不截图、不引入第二套排版。
- `gpui-kit-markdown-editor`：可选单语言代码块预览接口只改变视图，保留原代码模型与序列化；源码模式复用原编辑几何，预览保留真实源码长度，完整文档导出强制预览且不带工具栏。
- `gpui-kit-markdown-editor`：按文档 revision 缓存标题块序列，大纲跳转按标题索引定位，活动标题同步只遍历标题块。

Mermaid 适配保留编辑器原有的 `code_ui` 模块、投影缓存和折叠按钮 bounds；自定义节点使用 `editor_host::BlockState`。预览点击在编辑器 entity 的 `update` 内设置焦点，避免通过 `read(cx)` 跨越可变 App 借用。

字体沿用 `resources/fonts`，中文公式缺字时才读取系统微软雅黑。Markdown PDF 使用 `krilla` 写入上述绘制操作；整篇 `.tex` 编译保持 `latexmk` 流程。
