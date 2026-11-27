# Polyclave v5 阶段 1B（主动管家主控引擎 + Tauri v2 桌面端与 Obsidian Prism 级动态光学引擎）实施计划

> **前置条件：** 阶段 1A（`2026-11-26-polyclave-v5-phase1a.md`）全部任务与真实 CLI 门禁验收通过。

**目标：**
1. 交付 `polyclave-hub`：双模 `HubModelProvider`（原生 API 流式 + 借用 CLI 增量续接双写归档）、只读看仓工具、记忆策展、`manager` 常驻顾问会话、`Admission / Ticket` 预算分诊循环。
2. 交付 `apps/polyclave-app/src-tauri`：Tauri v2 原生外壳（拉起 `polyclaved`、系统钥匙串 `keyring`、系统托盘、原生通知）。
3. 交付 `apps/polyclave-app/ui`：基于 `DESIGN.md` 与 `polyclave-v5-desktop-ui.fig` 的轻量 TypeScript 界面，**100% 净室原创实现 `RMvdesi/obsidian-prism` 全部核心动态光学特效**（不复制任何 GPL-3.0 源码），同步更新 `DESIGN.md` 并通过 `tsc --noEmit` 与无头浏览器 1440px / 375px 全项验证。

---

## Part A：`polyclave-hub` 主动管家主控引擎

### Task 1：`HubModelProvider` 与 `ApiHubProvider`（原生 API 流式推理）
- **文件：** `crates/polyclave-hub/Cargo.toml`、`crates/polyclave-hub/src/lib.rs`、`crates/polyclave-hub/src/provider.rs`、`crates/polyclave-hub/src/api_provider.rs`
- **核心实现：**
  - 从系统钥匙串读取绑定在固定 `provider_profiles.base_url` 的 API Key（严禁会话级改写 URL）。
  - 支持 DeepSeek `deepseek-flash`（默认）或 `deepseek-v4-pro` 等 OpenAI-compat SSE 流式解析 `reasoning_content` 与原生 `tool_calls`，增量 delta 推入 `tokio::sync::broadcast`。
  - 静态前缀拼装：`[系统角色指令 + <project_memory_brief> (≤2000 tokens)] → [rolling_summary] → [已固化历史轮次]`，末尾附动态 `Ticket / Run` 状态快照。
- **提交信息：** `feat(hub): add HubModelProvider trait and ApiHubProvider streaming`

### Task 2：`CliHubProvider`（增量 `--resume` 续接 + 双写归档 + 无损重建）
- **文件：** `crates/polyclave-hub/src/cli_provider.rs`
- **核心实现：**
  - 基于 `hub_provider_threads(session_id, profile_id, agent_thread_id, last_synced_msg_id)`：同提供方连续对话直接 `--resume` 仅喂入增量消息；跨提供方切回时补齐缺席期间差量消息。
  - 每轮结束将完整回复、思考摘要与调度意图双写回 `polyclave.sqlite` `messages` 表；外部线程损坏时基于 `rolling_summary` + 近期 `messages` 无损重建。
  - **Claude CLI 规则：** 传 `--tools ""` 剥离全部内置写/Shell 工具，仅以 `--allowedTools` 放行宿主只读/调度/记忆 MCP；订阅登录模式**绝不传 `--bare`**。
- **提交信息：** `feat(hub): implement CliHubProvider incremental resume and canonical dual-write`

### Task 3：受控只读看仓工具、记忆策展与常驻顾问会话
- **文件：** `crates/polyclave-hub/src/repo_tools.rs`、`crates/polyclave-hub/src/curation.rs`
- **核心实现：**
  - 进程内只读看仓工具：`list_repo_files`、`read_repo_file`、`search_repo`、`get_run_diff`（强制 `dunce::canonicalize` 校验落在已登记 `repo_cwd` 内，硬编码拒绝 `.env*`、私钥、`.git/config` 与大二进制）。
  - 记忆策展：`memory_commit`（`candidate → verified`）、`memory_supersede`（`supersedes_id` 版本链，永不物理覆写）、`memory_archive`。
  - 注册 `manager` 常驻顾问会话，共享只读看仓与记忆工具，禁止直接写仓。
- **提交信息：** `feat(hub): add read-only repo tools, memory curation, and manager sessions`

### Task 4：主动管家事件分诊循环与 `Ticket / Mandate` 预算护栏
- **文件：** `crates/polyclave-hub/src/triage.rs`
- **核心实现：**
  - 三类入站事件：`explicit`（用户消息）、`worker_receipt`（Run 终态/阻塞回执）、`inferred`（仓库/系统信号，受 `admission_policies` `off | observe | auto` 约束并隔离外部 Bot 账号防自激）。
  - `BEGIN IMMEDIATE` 事务原子校验 `ticket.status == 'active'`、`ticket.repo_cwd == run.repo_cwd`、`runs_used < max_runs` 后才允许 `dispatch_task`；超限自动转 `blocked_on_user`。
  - 离线 `fake-llm-server` 集成测试覆盖预算拦截、越仓拒收与自动接力。
- **提交信息：** `feat(hub): implement proactive event triage loop and ticket budget guardrails`

---

## Part B：`apps/polyclave-app/src-tauri` 原生桌面外壳

### Task 5：Tauri v2 壳层、守护进程保活与系统钥匙串
- **文件：** `apps/polyclave-app/src-tauri/Cargo.toml`、`apps/polyclave-app/src-tauri/src/main.rs`、`apps/polyclave-app/src-tauri/src/keyring.rs`
- **核心实现：**
  - 启动时探测 `127.0.0.1:4310`，未运行则拉起 `polyclaved`（关闭窗口不断开后台守护进程）。
  - 读取 `local-auth.token` 注入前端授权上下文；通过 `keyring` crate 读写 API Key，前端仅可见 `configured: bool` 与掩码尾号。
  - 集成系统托盘驻留与桌面原生通知（`needs_review` / `blocked_on_user` 提醒）。
- **提交信息：** `feat(app): add Tauri v2 desktop shell, daemon lifecycle, and keyring integration`

---

## Part C：`apps/polyclave-app/ui` 光学界面与净室原创动态特效引擎

> **重要说明：** `polyclave-v5-desktop-ui.fig` 记录的是静态光学视觉规范；所有借鉴自 `RMvdesi/obsidian-prism` 的**动态 UI 特效**均在以下 `ui/src/motion/` 模块中以 **100% 净室原创 TypeScript + Web APIs** 实现（零第三方运行时依赖，不复制任何 GPL-3.0 源码）。

### Task 6：三栏光学布局、双主题 Token、SSE 客户端与核心视图
- **文件：** `apps/polyclave-app/ui/package.json`、`apps/polyclave-app/ui/tsconfig.json`、`apps/polyclave-app/ui/index.html`、`apps/polyclave-app/ui/src/styles/tokens.css`、`apps/polyclave-app/ui/src/styles/layout.css`、`apps/polyclave-app/ui/src/api/sse.ts`、`apps/polyclave-app/ui/src/state/focus.ts`、`apps/polyclave-app/ui/src/views/`
- **核心实现：**
  - **统一三栏 + 底栏：** `276px / 1fr / 432px` + `34px` 半透底栏；`<860px` 切换三标签响应式视图。
  - **光学主题系统：** 默认 **深空黑曜（Deep Obsidian `#0D0D10`）** 与 **白昼冷晶（Daylight Crystal `#F4F7FB`）**（预留暖炭玄岩 `#141413` 与暖白画册 `#F6F4EE`）；左/中/右三栏背景透明以透出底层全局极光，右栏底部核查/详情卡片采用微透烟熏玻璃质感（`rgba(20,20,24,0.82)` / `rgba(255,255,255,0.86)`）。
  - **数据与视图：** `fetch` + `ReadableStream` SSE 解析器（带 `Authorization: Bearer`、`Last-Event-ID`、`resync_required` 全量对齐）、`withFocusPreserved`、右栏四标签（`拓扑 DAG / 核查 Diff / 文件 / 跨项目记忆 Memory`）、底栏双环额度微组件（外环 5h / 内环周额度）与 `Settings · 用量与额度`（Agent HUD + Cost Meter + 512K 试管）。
- **提交信息：** `feat(ui): build 3-column optical layout, dual themes, SSE client, and core views`

### Task 7：净室原创动态光学与物理动效引擎（`ui/src/motion/`）
- **文件：** `apps/polyclave-app/ui/src/motion/spring.ts`、`scheduler.ts`、`aurora.ts`、`ink-stream.ts`、`smooth-caret.ts`、`vial.ts`、`pet-stage.ts`、`icons.ts`
- **7 大动态特效完整实现方案：**
  1. **四档二阶阻尼弹簧引擎（`spring.ts`）：** 求解阻尼谐振子方程，将 `snappy (k=420, c=30, m=0.9)`、`bouncy (k=380, c=20, m=1)`、`default (k=380, c=26, m=1)`、`gentle (k=260, c=26, m=1.05)` 预采样为 40 点 CSS `linear(...)` 函数并驱动 `Web Animations API`；提供 4 档动效开关（`off` / `subtle` / `standard` / `ultra`）并自动响应 `prefers-reduced-motion: reduce`。
  2. **分频节能帧调度器（`scheduler.ts`）：** 统一管理全部动态 Canvas/RAF 订阅——交互与拖拽爆发 `60–120fps`、推理稳态期限频 `~30fps`、空闲 `1.2s` 自动休眠至 `0fps`（将离屏/特效 Canvas 缩放至 `0×0` 释放显存，定格语义余晖）。
  3. **全局底部极光着色器（`aurora.ts`）：** 在根容器最底层挂载全视口宽固定定位 `<canvas>`（`position: fixed; inset: auto 0 0 0; height: 380px; z-index: 0; pointer-events: none`），横跨左栏、中栏、右栏与半透底栏；通过 WebGL2 Fragment Shader（含 Canvas 2D 渐变回退）渲染钴蓝/电离青/鸢尾紫地平线极光柔光波浪，运行/思考时 `~30fps` 流动，空闲自动降频至 `0fps` 静态余晖。
  4. **五级水墨流式淡入（`ink-stream.ts`）：** 利用浏览器原生 `CSS.highlights`（`Highlight` + `Range`，分 `16% / 38% / 62% / 84% / 100%` 五个透明度伪元素桶），在流式 token 到达时零 `<span>` DOM 包裹、零强制同步回流地实现墨迹渐显。
  5. **丝滑漂移光标（`smooth-caret.ts`）：** FastDOM 读写分离，监听七彩棱镜输入框输入/选区变化，驱动自定义光学竖线光标以 `snappy` 弹簧平滑插值跟随。
  6. **512K 高硼硅浅水流体试管（`vial.ts`）：** 24 节点 1D 浅水波动 + 重力摆物理模拟，输入与上下文增长触发微液滴激波，按占用率切换三态光谱（充裕蓝 / 充盈青 / 91% 预警暖橙），`1.1s` 无波动后自动休眠至 `0fps`。
  7. **四宠像素舞台与微动效图标（`pet-stage.ts` & `icons.ts`）：** 在输入框顶缘渲染 4 只原生像素精灵——**DeepSeek `Deepy`（无脚钴蓝鲸，配 30fps 动态喷水水花与尾鳍轻摆）**、**Codex `银白小龙`**、**Claude `Clawd`**、**Gemini `小紫猫`** 与右顶角 **像素棱镜钻石**；实现刚体防重叠、跳蛙与叠叠乐（totem stacking），同步响应运行/思考/待核查（`!` 弹跳）/空闲休眠（`zZ` at `0fps`）；配套 21 个微动效 SVG 图标、Cut-Crystal `Auto` 滑块、七彩角向渐变发送环与三态 `0fps` 语义余晖。
- **提交信息：** `feat(ui): implement clean-room damped-spring motion engine, WebGL2 aurora, ink stream, vial, and pet stage`

### Task 8：设计文档同步、UI 类型门禁与双视口无头浏览器验收
- **文件：** `DESIGN.md`（同步更新为深空黑曜/白昼冷晶、光学灯条、全局底部极光与七彩棱镜规范）、`.github/workflows/check.yml`（追加 UI `tsc --noEmit`）
- **验收：**
  - 无头浏览器在 `1440×900` 与 `375×812` 下实测：全局底部极光横跨三栏无水平切线、`Deepy` 无脚喷水动画正常、流式 `CSS.highlights` 正常、空闲 1.2s 后调度器进入 `0fps` 休眠、`prefers-reduced-motion` 下全动效关闭且页面信息完整、零横向溢出、零外部网络请求。
- **提交信息：** `docs(ui): sync DESIGN.md and verify v5 desktop/mobile UI in headless browser`
