# Folio 原生排版与 PDF 导出

- `latex-rust 3.0.3`：增加宿主字体的字形度量回调，并在数学盒模型保留字形所属字体。回退字形不使用主数学字体的脚本替换与重音度量。Folio 只消费盒模型，不调用该库的图片输出后端。
- `gpui-pre 0.3.12`：增加文档绘制捕获，保留字形、原始文字簇、字体、图形、图片及裁剪；捕获期间不创建 GPU 字形纹理。独立文档布局可以覆盖视口裁剪，以包含未滚动到的内容。
- `gpui-pre-windows 0.3.12`：从 DirectWrite 取得实际字体文件和集合索引，供 PDF 字体嵌入及文字复制使用。
- `gpui-kit-markdown-editor`：增加使用同一快照、节点渲染器的完整文档视图，省略编辑选区、光标和视口裁剪，并记录分页使用的块边界。
- `rusty-mermaid-gpui`：基于 `base58ed/rusty-mermaid` 修订 `6907e0747822cc2ee7044250a065497ce7d8f3c0` 的 MIT GPUI backend，适配本项目 `gpui-kit 0.9.1`，解析／布局／Scene 与 viewport helper 固定为同一 Git 修订。Scene 通过 `Arc` 共享，绘制准备缓存路径和文本；非零旋转、非均匀缩放及路径准备错误明确上报。无 SVG／PNG／WebView 降级路径。
- `gpui-kit-markdown-editor`：可选单语言代码块预览接口只改变视图，保留原代码模型与序列化；源码模式复用原编辑几何，预览保留真实源码长度，完整文档导出强制预览且不带工具栏。

Mermaid 适配保留编辑器原有的 `code_ui` 模块、投影缓存和折叠按钮 bounds；自定义节点使用 `editor_host::BlockState`。预览点击在编辑器 entity 的 `update` 内设置焦点，避免通过 `read(cx)` 跨越可变 App 借用。

字体沿用 `resources/fonts`，中文公式缺字时才读取系统微软雅黑。Markdown PDF 使用 `krilla` 写入上述绘制操作；整篇 `.tex` 编译保持 `latexmk` 流程。
