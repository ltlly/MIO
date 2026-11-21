# 固定依赖补丁

根 `Cargo.toml` 通过 `[patch.crates-io]` 锁定以下 4 个 vendored 依赖补丁（均排除于产品 workspace 成员之外，仅保留完整发布包源码、测试、清单与 Apache-2.0 许可证，不修改 Cargo registry 缓存）：

- `gpui-base` `0.9.0`（SHA-256 `cff6377e30f0c857681662e7edc7286bf30f5606033390401b0706d7bd930833`）：共修改 **20** 个文件
- `gpui-component` `0.9.0`（SHA-256 `45bbfdba0efa392190f2fd446ec311325302eb047d5ce45dc1bcf189cb18f5eb`）：共修改 **15** 个文件
- `gpui-pre-windows` `0.3.11`（SHA-256 `5141a3db59bab761a16ab19529fe4117a86e91922e8fc00af6dfc4829cb1744d`）：共修改 **2** 个文件
- `gpui-pre-linux` `0.3.11`（SHA-256 `79cac5cbd3485c1e3db8eafd5dc69019188f5cb0a39846946af772efa386f624`）：共修改 **1** 个文件

后续升级上游依赖时，需逐项确认上游等价能力及 `mio-ui` 对应回归全部通过后再移除本地 patch。

---

## 1. `gpui-base 0.9.0`（20 个修改文件）

### 1.1 多 inline token 原子替换、编辑世代与 Undo 内存上限
- `src/input/base/inline_tokens.rs`：支持在一次原生编辑中原子插入/替换多个 inline token（`replace_range_with_content`、`pending_tokens`），沿用原有校验、Unicode/IME 边界及 `EditIntent::Atomic` 撤销/重做历史。
- `src/input/base/state.rs`：新增 `pending_tokens`、`replace_all_without_history`、只读同步世代 `document_revision()`、`has_marked_text()`、`is_selecting()`，以及下文软折行/行号/行高配置接口。
- `src/input/base/undo_manager.rs`：新增 `MAX_UNDO_TEXT_BYTES`（`16 MiB`）撤销栈文本字节预算，在合并与压栈后自动淘汰最旧事务，避免大报文编辑持续增长内存。

### 1.2 超长单行软折行子行虚拟化、额外断行边界与大行高亮
- `src/input/base/element.rs`：
  - 将 `MAX_HIGHLIGHT_LINE_LENGTH` 从 `10_000` 提升至 `256 KiB`，并对超限巨行按可见窗口着重着色。
  - 新增软折行子行虚拟化（`VIRTUALIZE_WRAP_SUB_ROWS_THRESHOLD = 64`、`VIRTUALIZE_WRAP_SUB_ROW_OVERSCAN = 8`），仅对可见逻辑子行区间整形并维护累积 `build_run_ends` / `runs_for_range_with_ends` 二分切片；新增 `GutterDigitWidthKey` 行号数字宽度缓存。
- `src/input/editor/display_map/display_map.rs`、`src/input/editor/display_map/wrap_map.rs`：支持透传并在文本编辑时同步偏移 `extra_wrap_boundaries`。
- `src/input/editor/display_map/text_wrapper.rs`：新增 `with_virtual_window`、`measured_wrap_with_extra`、`first_wrapped_row()`、`total_wrapped_rows()`、`leading_byte_offset()` 与 `extra_wrap_boundaries` 调整逻辑；空缩进不再重复测量。
- `src/input/editor/display_map/text_wrapper/measurements.rs`：将换行测量单次文本上限 `MAX_TEXT_BYTES` 由 `4 MiB` 收敛为 `512 KiB`。
- `src/input/editor/display_map/text_wrapper/wrap_width/tests.rs`：同步测试夹具的 `extra_wrap_boundaries` 字段。
- `src/input/editor/indent.rs`：滚动缩进高度计算改用 `line_layout.total_wrapped_rows()` 以兼容子行虚拟化。

### 1.3 自定义行号槽、行号映射与精确行高
- `src/input/editor/highlighting.rs`：在 `InputHighlighter` 新增 `extra_wrap_boundaries` 默认方法，并定义 `EditorGutterStyle` 结构体（最小位数、最小宽度、左右边距、右边框与文本左内边距）。
- `src/input/base/kind.rs`、`src/input/editor/mod.rs`、`src/input/mod.rs`：在编辑配置、状态与公共重导出中接线 `gutter_style`（`EditorGutterStyle`）、`line_number_mapping` 与 `precise_line_height`。

### 1.4 受限正则搜索与流式 `RopeReader`
- `src/input/editor/search.rs`：新增受限正则匹配（`REGEX_SIZE_LIMIT = 1 MiB`、`REGEX_NEST_LIMIT = 128`、`update_regex_query` / `set_search_regex`）、IME 组词同步（`sync_search_matcher` / `reveal_current_search_match` / `clear_cached_text`），以及直接按分块流式扫描 `Rope` 的 `RopeReader`。

### 1.5 禁用态控件焦点路由与浮层边距
- `src/checkbox.rs`、`src/radio.rs`、`src/switch.rs`：已聚焦控件在异步保存禁用期间保留在焦点分派树中但移除 `tab_stop`，避免 Escape/Tab 键盘路由丢失。
- `src/popover.rs`、`src/popup.rs`：为弹出定位器暴露可配置的 `.margin(...)` 边界间距。

---

## 2. `gpui-component 0.9.0`（15 个修改文件）

### 2.1 表格委托选择、列操作与滚动条模式
- `src/table/delegate.rs`：在 `TableDelegate` 新增默认实现方法 `row_selection_managed_by_delegate`、`show_unsorted_sort_icon`、`sort_on_header_click`、`column_header_context_menu`、`hide_column`、`reset_columns` 与 `on_column_widths_changed`。
- `src/table/state.rs`：新增 `set_selected_row_without_scroll`、`set_selected_row_preserving_context`（保留右键上下文菜单目标）、`sort_column`、`copy_column`、`hide_column`、`auto_fit_column`、`reset_column_layout`、表头右键菜单与列宽/排序交互，以及字号自适应行高/表头高度计算。
- `src/table/data_table.rs`：新增 `.scrollbar_mode(...)` 控制表格滚动条显示策略。

### 2.2 编辑器/输入框内边距、行号配色与尺寸缩放
- `src/input/editor.rs`：为 `TextEditor` 新增 `.text_area_paddings(...)`。
- `src/input/input.rs`：为 `Input` 新增 `.text_area_paddings(...)`，同步主题行号颜色（`editor_line_number` / `editor_active_line_number`），并修正单行 `Size::Small` / `Size::XSmall` 高度及 `line_height(relative(1.25))` 垂直居中裁切。
- `src/input/mod.rs`：重导出 `EditorGutterStyle`。
- `src/sizing.rs`：补全 `Size::Size(_)` 的水平内边距与 `h(size)` 高度映射。

### 2.3 菜单定位、紧凑弹出菜单与绘制回退菜单
- `src/menu/dropdown_menu.rs`：新增 `offset`、`horizontal_offset`、`margin` 与 `trigger_style`。
- `src/popover.rs`：新增 `.margin(...)` 并透传给 `gpui-base` `Popover`。
- `src/menu/popup_menu.rs`：新增 `.compact()` 与 `.flush_swatches()` 模式，向子菜单继承紧凑尺寸并在紧凑模式下固定 `15px` 图标与 `23px`/`21px` 行高。
- `src/native_menu/mod.rs`、`src/native_menu/fallback.rs`：新增 `NativeMenu::set_use_drawn_popup` 运行时开关，并将回退绘制菜单配置为紧凑、可滚动且按视口高度钳位。

### 2.4 按钮悬停覆盖、命令面板按键与开关配色
- `src/button/button.rs`：新增 `Button::hover(...)` 以允许调用方覆盖默认变体的悬停样式。
- `src/command/state.rs`：在 `render` 中触发 `on_query_changed` 同步，并转发 `input::{MoveUp, MoveDown, Enter, Escape}` 动作到列表选择/确认/取消。
- `src/switch.rs`：未选中滑块使用主题颜色 `cx.theme().foreground`（替换硬编码 RGB），并将轨道圆角固定为胶囊全圆角 `bg_height`。

---

## 3. `gpui-pre-windows 0.3.11`（2 个修改文件）

- `build.rs`、`src/directx_renderer.rs`：上游在未运行 Windows SDK `fxc.exe` 时会在运行时调用 `D3DCompileFromFile(env!("CARGO_MANIFEST_DIR")/src/*.hlsl)`，导致目标机器因缺失编译机源码路径而在 DirectX 初始化崩溃（`os error 3`）。本地补丁将 `fxc.exe` 改为可选预编译（`gpui_precompiled_shaders`），未预编译时改用编译期内嵌 HLSL 源码配合系统 `d3dcompiler_47.dll` 执行内存级 `D3DCompile`（Release 启用 `D3DCOMPILE_OPTIMIZATION_LEVEL3`）。

---

## 4. `gpui-pre-linux 0.3.11`（1 个修改文件）

- `src/linux/x11/client.rs`：允许在支持 `_GTK_FRAME_EXTENTS` 的 X11 窗口管理器（例如未开启独立合成器的 XFCE4）下同样启用客户端自绘标题栏（`client_side_decorations_supported = gtk_frame_extents_supported`），以匹配 MIO 零阴影矩形边框的跨平台自绘标题栏。

---

## 5. 回归验证归属

- 表格多选与委托回归位于 [`crates/mio-ui/tests/table_row_selection.rs`](../crates/mio-ui/tests/table_row_selection.rs)，多 inline token 原子替换回归位于 [`crates/mio-ui/tests/inline_token_content.rs`](../crates/mio-ui/tests/inline_token_content.rs)，其余补丁回归随 `mio-ui` 单元测试一并运行；四个 vendor crate 本身继续保持在产品 workspace 之外。
