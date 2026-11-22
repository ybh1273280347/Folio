# Folio AI 性能优化交接手册

本文用于新会话直接接手 Folio AI 链路性能优化。内容只描述已落地的事实、验证范围和下一步路径，不依赖之前的对话上下文。

本文以 `c251d70` 为基线，仓库状态、验证结果和后续计划均对应当时的快照。

## 1. 目标与边界

优化 `resources/pi-host` 到 Rust AI 面板之间的热路径，重点是：

- Pi 宿主流式事件的序列化、传输和重复处理。
- Rust 侧轮询、事件分派、状态消费和多会话调度。
- 长对话流式输出时的历史遍历、Markdown 解析和组件重建。

必须保持：

- 现有 AI 面板布局、文案、操作方式和用户可见交互。
- Pi JSONL 会话文件和 Folio 已保存应用数据的兼容性。
- 事件顺序、工具调用参数变化、快照恢复、取消和终态行为。
- 后台会话继续运行、会话路由、工具回复归属和项目隔离。

Node 与 Rust 之间的内部 JSONL 契约可以调整，但两端必须同步修改。不以丢弃中间状态换取表面速度。

验收门槛：

- 受影响路径有聚焦测试或可复现基准。
- `cargo check --locked --bin folio-gpui-prototype --tests` 通过。
- 相同工作负载下，改动前后指标差异超过基线波动；首字延迟和事件顺序不得回退。
- 每阶段从最新 `main` 开分支，使用中文提交说明，验证后合回 `main` 并保留提交历史。

## 2. 当前仓库状态

- 当前分支：`main`
- HEAD：`c251d70`（`merge: 合并 AI 事件与调度性能优化`）
- 本地 `main` 比 `origin/main` 领先 8 个提交，尚未推送。
- 写本文档前工作树干净，当前树编译通过。
- 之前未完成的虚拟列表实验已完全移除；`src/ai` 中没有 `ListState`、`splice` 或 `remeasure_items` 残留，`src/ai/render.rs` 与 HEAD 一致。

已合入 `main` 的提交：

```text
c251d70 merge: 合并 AI 事件与调度性能优化
2545343 性能：降低 AI 流式事件与空轮询开销
679dd24 merge: 合并 AI 流式渲染优化
1639472 性能：减少 AI 流式重绘的全文扫描
37382d0 merge: 合并 Pi 宿主流式事件优化
ec41636 性能：精简 Pi 宿主流式事件载荷
24c56f1 merge: 合并 Pi 宿主流式性能基线
a895298 性能：添加 Pi 宿主流式基准工具
```

## 3. 已完成并验证的优化

### 3.1 可重复的宿主流式基准

提交：`a895298`

新增 `scripts/pi-host-stream-benchmark.mjs`。它启动真实 Pi 宿主和本地 OpenAI-compatible SSE 服务，重复测量：

- 流式事件 JSONL 字节数。
- `message_update` 事件数量。
- 首文本时间。
- `agent_settled` 时间。

可用环境变量：

- `FOLIO_BENCH_ROUNDS`：轮数，默认 5。
- `FOLIO_BENCH_CHUNKS`：每轮文本片段数，默认 64。
- `FOLIO_BENCH_CHUNK_BYTES`：每片段字节数，默认 256。

首轮包含 SDK 初始化开销，比较首字延迟时应单独看后续稳定轮次。

### 3.2 精简 Pi 宿主事件载荷

提交：`ec41636`

改动：

- `resources/pi-host/host.mjs` 在 `message_update` 中把完整 `partial` 快照保存到会话记录，不再随每个增量事件转发。
- 实时事件只保留 `assistantMessageEvent` 的增量和当前工具调用 `toolCall`。
- 完整快照仍用于 `snapshot` 恢复和重连。
- 工具调用增量从累计快照的 `content[contentIndex]` 提取当前 `toolCall`。

行为验证：

- `resources/pi-host/test/host.test.mjs` 覆盖工具参数增量在生成结束前到达 Rust。
- 测试断言增量事件不含 `partial` 和完整 `message`。
- 快照仍能恢复正在运行的工具调用。

### 3.3 减少流式回答的全文拼接

提交：`1639472`

改动：

- `src/ai/render.rs:693` 在任务运行期间不再把所有 assistant 文本片段拼成完整回答；复制和导出按钮只在任务结束后使用该文本。
- `src/ai/session.rs` 的工具调用处理改为读取宿主转发的顶层 `toolCall`，不再从累计 `partial.content` 中反复定位。

保留行为：

- 流式文本和思考内容照常渲染。
- 工具参数变化仍实时进入对话。
- 结束后复制和导出内容不变。

### 3.4 降低空轮询和事件反序列化开销

提交：`2545343`

改动：

- `src/ai/manager.rs:512` 根据是否存在流式会话或待处理 UI，动态使用 35ms 或 70ms 轮询间隔。
- `src/ai/session.rs:215` 对 `message_update` 直接读取 `assistantMessageEvent` 并分派，避免先反序列化成 `SessionEvent` 再序列化回 JSON。
- `resources/pi-host/host.mjs` 不再在流式事件中携带未使用的 `usage`。

保留行为：

- 有事件时仍按序列号去重和排序。
- 后台会话继续运行。
- 工具调用和 UI 请求仍走原有归属逻辑。

## 4. 验证证据

### 4.1 宿主测试

命令：

```pwsh
npm --prefix resources/pi-host test
```

结果：28 项全部通过，0 失败。覆盖同时运行的多会话、实时快照、项目隔离、关闭/重开保留、删除保护、手动重命名优先级、固定会话、稳定请求前缀，以及工具参数增量。

### 4.2 Rust 编译检查

命令：

```pwsh
cargo check --locked --bin folio-gpui-prototype --tests
```

结果：通过。

### 4.3 宿主流式基准

命令：

```pwsh
node scripts/pi-host-stream-benchmark.mjs
```

本次结果（5 轮，64 段，每段 256 字节）：

| 轮次 | 字节 | 事件 | 首文本 ms | settle ms |
| --- | ---: | ---: | ---: | ---: |
| 1 | 49,785 | 66 | 270.73 | 276.75 |
| 2 | 49,785 | 66 | 36.64 | 39.88 |
| 3 | 49,785 | 66 | 37.36 | 40.01 |
| 4 | 49,785 | 66 | 33.34 | 35.86 |
| 5 | 49,785 | 66 | 36.14 | 38.35 |

`resources/pi-host/README.md` 记录的优化前基线约为 1.23 MB；当前相同负载约为 49.8 KB，事件数和恢复快照保持不变。

### 4.4 验证边界

以上是自动化验证。最终清理后没有重新做一次人工 UI 验收；当前代码与通过测试的提交一致。下一阶段修改渲染后，仍需实际走一次 Folio AI 对话和工具路径。

## 5. 已丢弃的内容

以下改动没有合入，也不应假定它们存在：

- 无临时 `Vec` 的 Rust 入站事件 drain：能编译，但没有独立测试或基准证明收益。
- Markdown 公式归一化的无定界符快速返回：能编译，但属于 Markdown 编辑器路径，不是本阶段 AI 面板热路径，未单独验证。
- AI 消息虚拟列表实验：未完成，已移除，没有残留代码。
- 相关临时文档和辅助脚本：已删除。

## 6. 下一步：长对话渲染

这是当前最明确、收益最大的剩余目标。不要先假设虚拟列表一定正确，先用基准定位成本。

### 6.1 先测量

构造代表性长对话：

- 至少 100 条历史消息。
- 包含 Markdown 标题、列表、代码块、公式和工具调用。
- 在一个长回答上持续流式更新。

分别测量：

- 每个 delta 从接收到状态更新的耗时。
- 每次重绘遍历的历史消息数量。
- Markdown 解析耗时。
- 组件创建和布局耗时。

如果主要成本是重复解析未变化的消息，优先做按消息 ID 和内容版本缓存，而不是直接上虚拟列表。

### 6.2 虚拟列表路径

如果测量确认成本来自渲染全部历史消息，再使用 GPUI `ListState`：

- 列表只创建可见行。
- 行使用缓存的独立 View Entity，不要在每个 delta 中重建。
- 行 View 持有 `ChatPane` Entity，事件通过 `Window.listener_for` 指向它。
- 不要调用不存在的 `window.root_view()`。
- 不要在 List 行渲染 callback 中重入更新正在 render 的 `ChatPane`。
- 追加消息用 `splice`。
- 流式尾行用 `remeasure_items`。
- 会话快照变化或恢复时重置列表状态。

必须保留：

- 左侧消息导航刻度和悬停预览。
- 引用选择。
- 右侧滚动条和阅读位置标记。
- 点击跳转并暂停尾部跟随。
- 流式尾部自动跟随。
- 工具调用、思考块和确认对话框的行内交互。

验收重点：

- 长对话滚动流畅，不出现空白行或重复行。
- 流式尾行持续更新，不导致整段历史重排。
- 快照恢复后消息顺序和滚动位置正确。
- 多会话切换不串流。
- 取消后不继续追加已取消内容。

## 7. 关键代码位置

- `resources/pi-host/host.mjs:229`：`message_update` 载荷裁剪和 `record.partial` 保存。
- `src/ai/manager.rs:512`：自适应轮询间隔。
- `src/ai/session.rs:215`：入站事件 drain 和分派入口。
- `src/ai/render.rs:693`：流式期间延迟拼接完整回答。
- `scripts/pi-host-stream-benchmark.mjs`：宿主流式基准。

## 8. 接手检查清单

- [x] `main` 包含 4 个 AI 优化提交及对应合并提交。
- [x] 工作树在写本文档前干净。
- [x] `cargo check --locked --bin folio-gpui-prototype --tests` 通过。
- [x] `npm --prefix resources/pi-host test` 28/28 通过。
- [x] 宿主流式基准已复测并记录。
- [ ] 下一阶段从最新 `main` 开分支。
- [ ] 下一阶段完成后做一次实际 UI 对话和工具路径验收。

## 9. 工作方式提醒

- 先读 `ARCHITECTURE.md` 和本手册，再定位代码。
- 聚焦修改，不顺手重构无关模块。
- 不要测试 vendor 第三方 crate；它们不属于本次优化范围。
- 编译失败时修到通过，不要因为迁移未完成就回滚整个方向。
- 每阶段只跑与改动相关的聚焦检查，不重复跑全量测试。
- Node 和 Rust 的协议改动必须成对提交。
- 未实际运行验证的路径必须在交付时明确标注。
