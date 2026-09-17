# UIA 标题栏按钮对齐——实现方案（已实施）

> 状态：已实现并验证（2026-09-16）。实现在 `crates/tempmon-ui/src/main.rs`
> `uia_ensure_request` / `uia_query_close_y` / `uia_cached_close_y` /
> `App::align_anchor_top`，截图启发式 `strip_button_band_center_y` 保留为回退。
> 新增坑：浏览器标签页的关闭按钮同名"关闭"，名字命中后必须做右上角区域校验；
> 微信内置浏览器窗口常以隐藏态存在（ShowWindow 后可测）。

## 背景

当前 y 对齐（与前台窗口标题栏按钮垂直居中）靠截图边缘密度启发式：
`strip_button_band_center_y`（crates/tempmon-ui/src/main.rs）+ 梯式带宽
48/96/200px + 自顶向下首簇 + 绝对阈值 5。已在本机调通
（360 极速/360 文件夹/微信内置浏览器/ZCode），但阈值全是本机调参，
换机器换程序就可能失准——分发场景不可持续。

## 目标

用 Windows UI Automation 读取前台窗口标题栏按钮的**真实矩形**，
拿"关闭"按钮的 y 中心做对齐锚点；UIA 查不到时回退现有截图启发式。

## 实现要点

1. **依赖**：windows crate 开 `Win32_UI_Accessibility` feature
   （已有，SetWinEventHook 在用）；UIA 走 COM：
   `CoCreateInstance(CUIAutomation)` → `IUIAutomation` →
   `ElementFromHandle(前台 hwnd)`。
2. **找按钮**：`FindFirst(TreeScope_Descendants, property condition
   Name 含"关闭"/"Close" && ControlType == Button)`。中英文都要匹配
   （Name 依系统语言），匹配不到再退而求其次：取窗口右上角区域
   （x > right-200, y < top+60）内 ControlType==Button 的最右一个。
3. **超时与缓存**：UIA 树遍历可能几十~几百 ms，禁止在 UI 线程同步调——
   放独立线程，结果（按钮中心 y + 进程名/窗口类）写缓存
   （`Mutex<HashMap<isize /*fg hwnd类*/, i32>>`，TTL 60s）；
   主线程 tick 里只读缓存。
4. **失败回退**：UIA 结果为 None（元素不存在/超时 >500ms）时，
   走现有 `strip_button_band_center_y` 截图启发式，行为与现在完全一致。
5. **接入点**：`title_sync_from_strip` 与 phase2 的 ty 计算——
   优先 UIA 缓存值：`ty = uia_center - ctx.h / 2`；
   投票表决（sync_ty_hist 多数表决）逻辑保留，同样适用于 UIA 值。
6. **验证场景**（本机都有）：360 极速浏览器（自绘 tab 栏）、
   360FileBrowser64（360 文件管理器）、微信内置浏览器（WeChatAppEx）、
   ZCode（Electron，按钮左侧留白大）、Explorer（标准标题栏）。
   每个场景：切换前台 → ≤2s 内胶囊中心与按钮中心差 ≤2px。

## 已知坑

- Chromium 系窗口的标题栏按钮默认可能不进 UIA 树，
  需要窗口开启过屏幕阅读器标记（可给目标窗口发
  `WM_GETOBJECT` OBJID_CLIENT 探测；或用
  `UIA_Execute` 前提不到就回退启发式）。
- 管理员权限的程序（温度计以管理员跑）UIA 访问低权限窗口没问题，
  反向才有 UIPI 限制——本程序场景无碍。
-别把 UIA 调用放进 0.3s burst 补查的同步路径里。

## 相关代码

- `crates/tempmon-ui/src/main.rs`：`title_sync_from_strip`、
  `strip_button_band_center_y`、`dodge_if_occluding`（sync 门）、
  `sync_ty_hist` 多数表决。
- 手动拖动 → `user_pinned=true`：x 尊重用户，y 仍需同步
  （`sync=true` 走 `title_sync_from_strip`）。
- debug 日志：TEMPMON_DODGE_DEBUG=1。
- 测试脚本（v1.3.3 已从仓库删除，见 git 历史）：tools/test_browsers.py、tools/activate_force.py、
  tools/measure_align.py。
