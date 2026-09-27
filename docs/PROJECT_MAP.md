# PROJECT_MAP — bubble（AI 任务冒泡球）

桌面常驻悬浮球：按会话追踪 Claude / Codex，面板上段「等你」（真的需要你，按会话一行）、
下段「最近」（24 小时内有动静、且项目 VSCode 窗口开着的会话，按项目合并成一行，在跑的带绿点）；
点一条切到那个项目的 VSCode 窗口并把那个会话调出来（侧边栏或标签）；在编辑器里答复过、或在该会话的编辑器标签上停留满 15 秒的「等你」会自动回到「最近」。

## 目录结构

```
vscodeDesktopNotifictaion/
├── bubble/              Tauri 应用（球本体）
│   ├── src/             前端（纯静态，无框架无 Vite）
│   │   ├── index.html   球 + 展开面板的结构
│   │   ├── styles.css   球/面板样式
│   │   └── main.js      交互：拖拽/展开/列表/切窗口/退出
│   ├── vscode-ext/      VSCode 配套扩展（bubble.bubble-companion）
│   │   ├── extension.js 感知侧边栏正显示的会话 → 窗口有焦点时每 5s 往 inbox 写 seen；监听 ~/.claude/bubble/open.json，工作区包含该会话目录的窗口认领并打开 Claude 会话
│   │   ├── test-extension.js 扩展自检（假 vscode 模块 + 临时 HOME，不碰真实 bubble 目录）
│   │   └── package.json `npm run install-local` = vsce 打包 + code --install-extension（.vsix 不入库）
│   └── src-tauri/       Rust 后端
│       ├── src/lib.rs   窗口收放、切窗口、托盘、命令、每 1.5s 调一次 sessions
│       ├── src/sessions.rs  会话状态机：inbox 事件 + Claude/Codex 记录扫描（含全部单元测试）
│       ├── src/focus_window.ps1  Win32 EnumWindows 按标题找 VSCode 窗口并强制置前（编译进 exe）
│       ├── tauri.conf.json  窗口配置（无边框/透明/置顶/76px）
│       ├── capabilities/default.json  前端权限（拖拽/事件）
│       └── Cargo.toml
├── hook/
│   ├── bubble-hook.js          Claude 钩子（UserPromptSubmit/Stop/Notification/SessionEnd）：往 inbox 写一条事件
│   ├── codex-bubble-notify.js  Codex notify（一轮结束）：往 inbox 写一条事件
│   ├── test-hook.js            钩子事件分类自检（临时 HOME，不碰真实 inbox）
│   └── test-origin.js          transcript 路径 → 会话起始目录 解析自检
└── docs/
    └── PROJECT_MAP.md   本文件

运行时数据（不在仓库里）：~/.claude/bubble/
├── inbox/       钩子写入、球消费的一次性事件文件（一事一文件）
├── state.json   球维护的会话列表（state: running / waiting / idle）
└── open.json    点 Claude 行时写的打开请求 {cwd, session_id, ts}，匹配的窗口认领后删除
```

## 数据流

球每 1.5s 做一轮 `poll`，四个来源按会话 id 合并进 state（`sessions.rs`）：
1. **inbox**：钩子事件。`start`（发出提示）→ 运行中；`stop`（答完）→ 交给记录扫描判断；`notify`（要权限 / Codex 答完）→ 等你，角标 +1、脉冲；`end` → 空闲。Codex 的 notify 带 `agent:"codex"`，只认已由 rollout 跟踪到的会话：Codex 扩展每次发提示都会另开一个无 rollout 的临时 thread 生成标题，它一答完也触发 notify，不过滤就会凭空多一条「等你」。
2. **Codex 记录** `~/.codex/sessions/**/rollout-*.jsonl`：文件变大才算活跃（Windows 上 Codex 开着文件写，修改时间不更新），增量读 `task_started / task_complete / turn_aborted`。
3. **Claude 记录** `~/.claude/projects/*/*.jsonl`：记录里最后一条的时间戳在 24 小时内（修改时间不可信，曾有批量改动）；每行记 `mark`（事件发生时的文件大小），之后出现对话条目 = 你在编辑器里处理了（等你 → 运行中），`stop_hook_summary` = 一轮结束，`[Request interrupted by user` = 中断（→ 空闲）。
   一轮结束时若 `<会话>/subagents/agent-<id>.jsonl` 有 2 分钟内在写、且主记录里还没有 `<task-id><id></task-id>` 完成通知的子 agent → 后台仍在工作，保持运行中，不进「等你」。

4. **扩展 seen 心跳**（`bubble/vscode-ext/`）：窗口有焦点时，对每个「正显示在界面上」的会话每 5s 一条 `{kind:"seen", cwd:工作区, session_id, since:max(开始显示, 获得焦点)}`。只清「等你」、从不建行；会话目录要在该工作区下，且 `ts - max(since, 进入等你的时间) ≥ 15s`（`SEEN_MS`）。「正显示」的来源：
   - Claude：本窗口自己的 `Claude VSCode.log`（`context.logUri` 的兄弟目录 `Anthropic.claude-code/`），webview 挂上某会话记 `update_session_state`，切走记 `isFarewell` / `panelNoLongerHosts`。
   - Codex：本机 IPC 管道 `\\.\pipe\codex-ipc`（帧 = 4 字节小端长度 + JSON），视图挂上某对话广播 `thread-stream-following-changed {conversationId, following}`；扩展只听不答（discovery 一律 `canHandle:false`）。

**窗口过滤**：每轮用 Win32 `EnumWindows` 取所有 `* - Visual Studio Code` 窗口标题；会话目录或其任一上级目录名匹配某个窗口才追踪（与 `focus_window.ps1` 同规则）。窗口没开的非「等你」会话直接不追踪，窗口打开后下一轮自动补上。
非「等你」的会话 24 小时没动静就移出；「等你」一直保留到处理。

点某行 → 按标题 `<项目名> - Visual Studio Code` 找到窗口并强制置前（找不到才 `code -n <cwd>`），同时打开会话：Claude → 写 `open.json`（`request_companion_open`），每个窗口的扩展都监听它，工作区包含会话目录的那个窗口删文件认领并调 `claude-vscode.editor.open`（不走 vscode:// 链接，所以没有「允许扩展打开此 URI」确认框）（按 `claudeCode.preferredLocation` 开在侧边栏或标签，本机已设 `sidebar`）；Codex → 置前后开 Codex 自带的 `vscode://openai.chatgpt/local/<id>`（`Task::session_link`，切它的侧边栏）；「等你」的行点完回到「最近」。× 把行藏起来，直到那个会话有新动静。

## 交互模型

- 球常驻；红角标只数「等你」；有会话在跑或在等你时球是彩色，否则变灰。「清空」把「等你」全部移回「最近」。
- 球固定在窗口右上角、收起/展开同尺寸同边距 → 展开时屏幕坐标不变，不跳位。
- 拖：原生 `data-tauri-drag-region`；单击（屏幕位移<5px）= 展开/收起。
- 收起 = 再点球 / 点到别处失焦(`onFocusChanged`) / Esc。
- 拖动位置每 1.5s 持久化到 `pos.json`，下次启动恢复。

## 写入路由（新东西放哪）

- 前端改动（UI/交互）→ `bubble/src/`
- 后端窗口/命令 → `bubble/src-tauri/src/lib.rs`；会话状态判断 → `bubble/src-tauri/src/sessions.rs`
- 编辑器内的感知/跳转（侧边栏正显示哪个会话、打开指定会话）→ `bubble/vscode-ext/extension.js`，改完 `npm run install-local` 并重载 VSCode 窗口
- 钩子采集逻辑 → `hook/bubble-hook.js`（Codex 完成通知 → `hook/codex-bubble-notify.js`）
- 临时/实验产物 → 不入仓库，放系统临时目录
- 运行时任务数据 → `~/.claude/bubble/`（代码里的常量，勿硬编码别处）

## 已知取舍（ponytail ceilings）

- 每轮轮询都遍历 ~1k 个 Claude 记录 + ~1k 个 Codex 记录的大小（实测约占单核 1%）。嫌高再降扫描频率或换文件系统 watcher。
- 活动时间取记录最后 8KB 里最后一个 `"timestamp"`（按文件缓存，修改时间变了才重读）。
- 窗口标题匹配不到的会话（多根工作区、远程窗口、带 Profile 名的标题）不会被追踪，也就不会提醒。
- 后台子 agent 卡在一个超过 2 分钟的工具调用里 → 会被当成已停，主会话提前进「等你」。
- 「运行中」的会话 20 分钟没有任何写入就当死掉移除（单个工具调用超过 20 分钟会被误移除，写入后自动回来）。
- Claude 的「一轮结束」靠 `stop_hook_summary`，前提是配置了 Stop 钩子（bubble 本来就要求）。
- 后台子 agent 要权限时，主 agent 仍在写记录 → 「等你」会被提前清掉（主 agent 答完还会再提醒一次）。
- Codex hooks.json 的钩子在它的沙箱里写不了文件、也不触发 UserPromptSubmit（2026-09-24 实测），所以 Codex 靠记录扫描 + notify，不靠 hooks.json。
- 只扫 `~/.claude/projects`；其它 CLAUDE_CONFIG_DIR 账号只靠钩子。
- 切窗口靠窗口标题匹配 `<项目名> - Visual Studio Code`：若两个项目**目录同名**会命中错的那个。要精确可改成存 workspace 唯一标识。窗口已关则 `code -n` 开新窗口。
- 列表显示项目名 + 最近一条提示的首行（或权限提醒）+ 时间，不做摘要。
- 「停留 15 秒」靠两个扩展的内部细节：Claude 的 info 级日志行 `Received message from webview:`、Codex 的 IPC 帧格式。任一扩展升级都可能让它静默失效（点行、答复照常）。
- 「正显示」= 挂在侧边栏上：侧边栏收起、或辅助栏当前显示的是另一个容器（Codex 盖住 Claude），停留也照样算。
- 关掉 Claude 编辑器标签不写 farewell，被关的会话会一直被当成「正显示」直到窗口重载。
- Codex 点行靠「先置前窗口、300ms 后开 vscode:// 链接」，VSCode 把链接交给最后激活的窗口；窗口没开（走 `code -n`）时不开链接。
- Claude 的 `open.json` 30 秒内有效：`code -n` 新开的窗口扩展启动时会补开；启动超过 30 秒就不开。父子目录各开一个窗口时，先认领的窗口打开（不一定是更深的那个）。
- Codex 新 thread 若在创建后 1.5s 内就答完第一轮（还没被 rollout 扫描建行），这轮不提醒。
- 窗口 `resizable:false`（必须）：Windows 对可调整大小窗口有最小宽度(~SM_CXMIN 136px)，会把 72px 的球撑大挤出屏幕。程序化 set_size 不受影响。
