# GLM 桌面温度计

钉在桌面右上角的半透明系统监控小部件。永远置顶、鼠标穿透不挡操作，全屏看视频自动隐藏，全屏游戏照常显示。

```
CPU 8% 62° │ GPU 2% 50° VR 13% │ MEM 50% │ DISK 60° 37° │ FAN 1951 1200
```

![预览说明：单行条悬浮于屏幕右上角，半透明圆角，浅色文字](docs/preview.md)

## 功能

| 类别 | 内容 |
|---|---|
| CPU | 占用率 + 温度（Intel Package / AMD Tctl，自动识别） |
| GPU | 核心占用 + 温度 + 显存占用（NVIDIA NVML，A 卡走 ADLX 兜底） |
| 内存 | 占用率 |
| 硬盘 | NVMe / SATA 温度（最多两块盘） |
| 风扇 | 主板 SuperIO 风扇转速（最多三个） |
| FPS | 前台应用实测帧率（PresentMon， experimental，默认关闭） |

## 形态

- **完整条**（24px 高）：单行显示全部段位，等宽槽位右对齐，宽度稳定不跳动
- **小胶囊**（24px 高）：按硬件分组翻页（CPU/GPU/内存/硬盘/风扇），轮播间隔 1-5 秒可调，滚轮可直接翻页
- 左上角几何色点实时反映系统健康状态（负载+温度综合四档：绿/黄/橙/红）

## 操作

| 操作 | 功能 |
|---|---|
| 默认状态 | 鼠标穿透，不挡任何操作 |
| 按住 Ctrl 悬停 | 解除穿透，进入编辑态 |
| Ctrl + 左键拖拽 | 移动位置（自动持久化） |
| Ctrl + 右键 | 弹出设置菜单 |
| Ctrl + 点击胶囊 | 展开为完整条 |
| Ctrl + 滚轮 | 胶囊翻页 |

## 菜单（Ctrl + 右键）

```
收起为小胶囊 / 展开为完整条
────────────────
外观 ▸    背景颜色（深色/浅色/深色毛玻璃/浅色毛玻璃）
          透明度（25%/50%/70%/90%）
────────────────
显示内容 ▸   CPU/GPU/内存/硬盘/风扇 五段开关
胶囊显示项 ▸  最高温度/CPU占用/CPU温度/GPU占用/GPU温度/显存/内存/硬盘/风扇
────────────────
轮播间隔 ▸   1/2/3/5 秒
刷新频率 ▸   500ms/1s/2s
────────────────
开机自启
恢复吸附右上角
────────────────
退出
```

所有设置即改即存（`%APPDATA%\tempmon.conf`），重启自动恢复。

## 架构

```
tempmon.exe (UI)          置顶窗口渲染 + 显隐策略 + 菜单
  └─ tempmon.exe (sensor) 采集子进程：CPU/GPU/内存/硬盘 → 共享内存
       ├─ lhm-bridge.exe  CPU 温度 + 风扇转速（LibreHardwareMonitorLib）
       └─ PresentMon.exe  FPS 测量（实验性，默认关闭）
```

三级进程隔离：任何一级崩溃都会被看门狗自动重启，UI 不受影响。

## 构建

依赖：Rust (MSVC)、.NET 8 SDK（仅 lhm-bridge 需要）

```bash
cargo build --release
cd lhm-bridge && dotnet publish -c Release
# 复制 lhm-bridge 发布产物到 target/release/
```

## 数据来源

| 数据 | 通道 |
|---|---|
| CPU/GPU/内存占用 | Windows PDH 计数器（免驱动） |
| GPU 温度/风扇 | NVIDIA NVML（驱动自带） |
| CPU 温度 | lhm-bridge（LibreHardwareMonitorLib，需签名驱动） |
| 硬盘温度 | Get-StorageReliabilityCounter（系统存储栈，免驱动） |
| 显存占用 | PDH GPU Adapter Memory |

## 已知限制

- FPS 显示为实验性功能（PresentMon CSV 缓冲机制限制），默认关闭
- WebView 类应用（豆包等）的 FPS/GPU 数据可能归属子进程
- 内存温度未实现（多数消费级主板无此传感器）
- 毛玻璃模式圆角弧度由系统控制（约 8px）

## 许可

仅供个人学习使用。
