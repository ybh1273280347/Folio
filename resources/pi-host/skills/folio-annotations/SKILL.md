---
name: folio-annotations
description: 查看、添加、修改或删除 Folio 项目中 Markdown 或 PDF 的批注。用户要求批注、评注、圈出问题或给文档加阅读提示时使用；不用于改写正文、PDF 原生注释导出或 Word 评论。
---

# 在 Folio 中写批注

使用 `folio_annotations_list` 查看已有批注，使用 `folio_annotation_create` 添加批注。参数以工具定义为准；不要直接编辑 `.folio/annotations.json`，也不要自行拼接 `<mark>`。

- 先阅读目标内容，再选择有价值的少量位置写具体评论：解释难点、指出推理缺口、联系上下文或提出明确修订建议。
- PDF、Word 等宏观阅读优先使用 `convert-documents-to-markdown`，Markdown 产物可用 `read-markdown` 有针对性地读取。转换产物用于理解内容，不是批注坐标。
- `quote` 必须是原文的可读文字。Markdown 选单个段落、标题或表格单元格内的片段，不带外层 Markdown 格式符号，不跨块，也不引用图片或公式占位符。
- PDF 必须确认从 1 开始的实际文件页码。书上印刷页码、转换后的 Markdown 行号和章节编号都不能直接当作工具的 `page`。
- 重复原文导致 `ambiguous_match` 时，增加能明确定位的原文上下文，或在确知位置后指定返回候选中的 `occurrence`；不要默认选第一处。
- `quote_not_found` 时重新核对该页或该文字块的原文。不要连续盲猜，不切换成坐标、OCR 或直接修改存储文件。
- 用户要求修改或删除时，先用 `folio_annotations_list` 查看该文件的批注，按原文、位置和评论确定目标 ID。使用 `folio_annotation_update` 替换评论，空字符串可清空评论并保留高亮。使用 `folio_annotation_delete` 同时移除批注与高亮。每次写入后需要继续操作时，以最新列表确认目标 ID。
- 添加工具返回已有不同评论时保留原评论。只有用户要求修改该条批注时才调用修改工具。
- 遇到 `document_changed`，重新读取内容后再决定是否重试；`project_not_active` 时请用户切回原项目。
- 扫描页、图片区域、复杂公式、跨块定位，以及写入 PDF/Word 原生评论不在首版范围内。说明未完成的部分。
- 只有工具返回成功后才声称相应操作已完成。若工具明确报告正文高亮和批注记录的结果不同，应准确说明部分完成的状态。
