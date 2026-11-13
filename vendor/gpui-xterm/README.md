# gpui_xterm

`gpui_xterm` 是一个可复用的 GPUI 终端组件，提供接近 xterm 的终端渲染、输入、选择、剪贴板与事件回调能力。

## 特性

- 基于 `alacritty_terminal` 处理终端状态与 ANSI 序列
- 提供可嵌入 GPUI 应用的 `TerminalView`
- 支持键盘输入、鼠标选择、滚动与剪贴板
- 支持标题、铃声、退出、尺寸变化等回调
- 支持颜色主题与基础渲染配置
- 块元素与常用单线、粗线、双线框字符按物理像素绘制，跨字体和行距保持连接

## 安装

```toml
[dependencies]
gpui-kit = "0.9.0"
gpui_xterm = "0.3.0"
```

## 主要导出

- `TerminalView`
- `TerminalConfig`
- `TerminalState`
- `TerminalRenderer`
- `ColorPalette`
- `GpuiEventProxy`
- `TerminalEvent`

## 字符画渲染

单元格宽高根据主等宽字体测量，字体或字号变小时会重新计算。
Unicode 块元素（U+2580–U+259F）及常用直线框字符使用单元格几何绘制，
不依赖回退字体的字形留白；其他字符继续使用字体渲染。
`line_height_multiplier` 只对行高应用一次，块元素填满对应单元格高度。

可运行字符画样例检查 banner、ASCII 图案、块元素和线框：

```sh
cargo run --example character_art
```

可选环境变量：`GPUI_XTERM_FONT`（字体名称）、`GPUI_XTERM_FONT_FILE`（字体文件）、
`GPUI_XTERM_LINE_HEIGHT`（行高倍率，默认 1.2）和 `GPUI_XTERM_LIGHT=1`（浅色背景）。

## 许可证

本项目基于 MIT License 发布。详见 `LICENSE`。
