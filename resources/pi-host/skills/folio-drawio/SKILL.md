---
name: folio-drawio
description: 在 Folio 中创建或修改 Draw.io 图表。用户要求画流程图、架构图、时序图，或希望调整当前图中的某些元素时使用。
---

# 在 Folio 中画 Draw.io 图

先弄清楚用户想表达什么，再选合适的图形结构。图表应当易读、层次清楚，元素和连线的位置要服务于内容；除非用户有现成规范，不必追求复杂装饰。

## 修改当前图

用户正在 Draw.io 中选中元素并提出修改时，先调用 `folio_drawio_get_selection`。用返回的 cell ID 定位对象，再通过 `folio_drawio_edit_cells` 一次提交相关操作。这样可以保留用户手工调整过的其他部分。

操作示例：

```json
{
  "operations": [
    { "op": "update", "id": "8", "value": "认证服务" },
    { "op": "update", "id": "8", "geometry": { "x": 240, "y": 120 } },
    { "op": "add_vertex", "id": "folio-auth-db", "value": "令牌库", "x": 430, "y": 120, "width": 120, "height": 52 },
    { "op": "add_edge", "id": "folio-auth-db-edge", "value": "读写", "source_id": "8", "target_id": "folio-auth-db" },
    { "op": "delete", "id": "9" }
  ]
}
```

新增顶点时给出简洁的标签和合适的尺寸。新增连线时引用已有顶点或同一批次中先创建的顶点。更新只填写需要变化的字段，几何位置可以只改 `x`、`y`、`width` 或 `height` 中的一部分。

如果用户没有选中元素但明确说要修改当前图，先读取选区确认当前状态。选区为空时，不要猜要改哪个 cell；可根据用户提到的名称调用 `folio_drawio_read` 读取指定文件，找到目标 cell ID 后再编辑。找不到唯一目标时，问一个简短问题确认对象。

只有在用户要大幅重组、整图重画，或局部操作表达不清时，才考虑用 `folio_drawio_update` 替换完整 XML。整图更新前先读取目标文件，并保留用户已有内容中仍符合要求的部分。

## 新建图表

用户要求创建新图时，优先用 `folio_drawio_create` 创建项目中的 `.drawio` 文件。XML 使用 Draw.io 原生格式，根元素为 `mxfile`，每个 cell 有唯一 ID，连线的 `source` 和 `target` 指向图中的顶点。创建后可通过 Folio 文件打开目标图表。

用户只要求调整已打开画布时，不要另建文件，也不需要先重建完整 XML。编辑工具成功后，简洁说明哪些内容已在画布中更新，不必把正常操作写成检查报告。
