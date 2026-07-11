# PROJECT_MAP — bubble（AI 任务冒泡球）

桌面常驻悬浮球：把每次 Claude/agent 任务结束当成一条待处理任务，攒在球上，
点开可交互查看、按项目分组，点一条直接 `code <cwd>` 切到那个 VSCode 窗口。

## 目录结构

```
vscodeDesktopNotifictaion/
├── bubble/              Tauri 应用（球本体）
│   ├── src/             前端（纯静态，无框架无 Vite）
│   │   ├── index.html   球 + 展开面板的结构
│   │   ├── styles.css   球/面板样式
│   │   └── main.js      交互：拖拽/展开/列表/切窗口/退出
│   └── src-tauri/       Rust 后端
│       ├── src/lib.rs   状态、inbox 轮询、切窗口、窗口收放、命令
│       ├── src/focus_window.ps1  Win32 EnumWindows 按标题找 VSCode 窗口并强制置前（编译进 exe）
│       ├── tauri.conf.json  窗口配置（无边框/透明/置顶/76px）
│       ├── capabilities/default.json  前端权限（拖拽/事件）
│       └── Cargo.toml
├── hook/
│   └── bubble-hook.js   Claude Stop 钩子脚本：往 inbox 写一条任务
└── docs/
    └── PROJECT_MAP.md   本文件

运行时数据（不在仓库里）：~/.claude/bubble/
├── inbox/       钩子写入、球消费的一次性事件文件（一事一文件）
└── state.json   球维护的当前待处理列表
```

## 数据流

Claude 任务结束 → Stop 钩子跑 `bubble-hook.js` → 写 `~/.claude/bubble/inbox/<ts>-<rand>.json`
→ 球每 1.5s 轮询 inbox、按 cwd 合并进 state → 有任务则 show 窗口、角标 +1、脉冲
→ 点球展开（面板向下-向左浮出，球右上角锚点屏幕坐标不变）→ 点某项目
→ 按标题 `<项目名> - Visual Studio Code` 找到已开窗口并强制置前
（AttachThreadInput 破前台锁；找不到才 `code -n <cwd>` 开新窗口）+ 从 state 移除
→ 任务数归 0 则 hide 整个窗口（闲时不可见）。

## 交互模型

- 有任务才显示，0 任务隐藏整个窗口（`apply_visibility`）。
- 球固定在窗口右上角、收起/展开同尺寸同边距 → 展开时屏幕坐标不变，不跳位。
- 拖：原生 `data-tauri-drag-region`；单击（屏幕位移<5px）= 展开/收起。
- 收起 = 再点球 / 点到别处失焦(`onFocusChanged`) / Esc。
- 拖动位置每 1.5s 持久化到 `pos.json`，下次启动恢复。

## 写入路由（新东西放哪）

- 前端改动（UI/交互）→ `bubble/src/`
- 后端逻辑（命令/状态/窗口）→ `bubble/src-tauri/src/lib.rs`
- 钩子采集逻辑 → `hook/bubble-hook.js`
- 临时/实验产物 → 不入仓库，放系统临时目录
- 运行时任务数据 → `~/.claude/bubble/`（代码里的常量，勿硬编码别处）

## 已知取舍（ponytail ceilings）

- 任务按 `cwd` 合并：同一目录跑多个并行 agent 会并成一行。要分开就改成按 session_id 建键。
- 轮询 1.5s，不用文件系统 watcher。嫌延迟再换 notify。
- 切窗口靠窗口标题匹配 `<项目名> - Visual Studio Code`：若两个项目**目录同名**会命中错的那个。要精确可改成存 workspace 唯一标识。窗口已关则 `code -n` 开新窗口。
- 列表只显示项目名+时间，不解析 transcript 摘要。要摘要是后续增强。
- 窗口 `resizable:false`（必须）：Windows 对可调整大小窗口有最小宽度(~SM_CXMIN 136px)，会把 72px 的球撑大挤出屏幕。程序化 set_size 不受影响。
- 闲时窗口隐藏 → 没任务时无法从球上退出。要退出得等有任务时用面板的「退出」，或杀进程。需要常驻退出口可加托盘/热键。
