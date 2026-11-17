---
name: folio-annotations
description: 在 Folio 项目中的 Markdown 或有文字层的 PDF 上添加原文高亮和评论。用户要求批注、评注、圈出问题或给文档加阅读提示时使用；不用于改写正文、PDF 原生注释导出或 Word 评论。
---

# 在 Folio 中写批注

使用 `folio_annotations_list` 查看已有批注，使用 `folio_annotation_create` 添加批注。参数以工具定义为准；不要直接编辑 `.folio/annotations.json`，也不要自行拼接 `<mark>`。

- 先阅读目标内容，再选择有价值的少量位置写具体评论：解释难点、指出推理缺口、联系上下文或提出明确修订建议。
- PDF、Word 等宏观阅读优先使用 `convert-documents-to-markdown`，Markdown 产物可用 `read-markdown` 有针对性地读取。转换产物用于理解内容，不是批注坐标。
- `quote` 必须是原文的可读文字。Markdown 选单个段落、标题或表格单元格内的片段，不带外层 Markdown 格式符号，不跨块，也不引用图片或公式占位符。
- PDF 必须确认从 1 开始的实际文件页码。书上印刷页码、转换后的 Markdown 行号和章节编号都不能直接当作工具的 `page`。
- 重复原文导致 `ambiguous_match` 时，增加能明确定位的原文上下文，或在确知位置后指定返回候选中的 `occurrence`；不要默认选第一处。
- `quote_not_found` 时重新核对该页或该文字块的原文。不要连续盲猜，不切换成坐标、OCR 或直接修改存储文件。
- 工具返回已有不同评论时保留它，不用文件编辑工具绕过覆盖限制。用户若要修改已有批注，说明首版只支持新增，可在 Folio 批注栏手动修改。
- 遇到 `document_changed`，重新读取内容后再决定是否重试；`project_not_active` 时请用户切回原项目。
- 扫描页、图片区域、复杂公式、跨块定位，以及写入 PDF/Word 原生评论不在首版范围内。说明未完成的部分。
- 只有工具返回成功后才声称批注已添加。若明确报告高亮已保存但评论保存失败，应准确说明这个部分完成状态。
