---
name: folio-canvas
description: 使用 Folio 原生知识画布（.canvas）工作。当需要读取、新建、扩展、整理思维导图或自由画布时使用，确保正确对齐自由/树形双视图与水平/垂直导图模式。
---

# Folio 知识画布（.canvas）

Folio 的 `.canvas` 画布是双视图原生白板（支持 `freeform` 自由画布与 `tree` 树形导图切换，且支持 `mindmap_right` 水平导图与 `tree_down` 垂直树状图）。

## 工作流

1. **先读后写**：修改已有画布前，先调用 `folio_canvas_read`（必要时结合 `folio_canvas_get_selection`）获取当前 `view_mode`、`tree_mode` 与 `revision`。不要盲改节点坐标或破坏现有结构。
2. **理解双视图规则**：
   - **`tree` 树形视图**：由布局引擎全权管理位置，新增节点只需指定 `parent_id`，**不要传手动 `x`/`y`**。可用 `view_mode: "tree"` 搭配 `tree_mode`（`mindmap_right` 水平导图适合概念发散， `tree_down` 垂直树状图适合自顶向下结构/流程）。
   - **`freeform` 自由视图**：保留用户手工排版。若需扩展局部导图分支，传入 `parent_id` 后可调用 `folio_canvas_layout` 并指定 `root_id`（不改全局 `view_mode`）仅排布该局部子树；若要构建对照表、矩阵或分组卡片，使用 `node_type: "group"` 与明确的 `x`/`y`/`width`/`height`。
   - **整图切入树视图**：调用 `folio_canvas_layout` 且 `view_mode: "tree"`（或整图 `layout`）前，画布会自动快照 `free_pos`，用户切回自由视图时原手摆坐标可无损恢复。
3. **节点类型与内容规范**：
   - 优先使用简洁短句作为文本卡片标题/要点（画布卡片采用居中排版，长段落应放在对应笔记文档并用 `anchor` 挂载引用，而不是整段塞进卡片）。
   - 插入图片或附件卡片时使用 `node_type: "file"`，可用 `text` 附带简短说明。图片的 `file` 从画布所在目录计算，例如 `maps/topic.canvas` 引用 `assets/diagram.png` 时填 `../assets/diagram.png`。其他项目文件使用项目相对路径。
   - 仅当明确需要引用笔记/PDF 原文时附带 `anchor`；没有对应工具动作的纯外部网址不要虚构节点类型。
4. **单事务批量提交**：尽量将同一轮创建的父子节点、分支连线打包进单次 `folio_canvas_apply_ops`（附带 `expected_revision`），这样用户的 `Ctrl+Z` 可以单步完整撤销。
