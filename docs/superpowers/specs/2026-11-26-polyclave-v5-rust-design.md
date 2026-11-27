# Polyclave v5 设计规范：全栈 Rust 守护进程 + Tauri v2 轻量中枢控制台

- 日期：2026-11-26
- 状态：v5 正式定稿设计规范（已完成两轮架构审查、纳入双层记忆系统，并完成桌面端 UI 与品牌 Logo 视觉定稿，源稿见 `docs/superpowers/design/polyclave-v5-desktop-ui.fig`；待指令后生成阶段 1A 实施计划）
- 取代：`docs/superpowers/specs/2026-11-25-agent-console-design-v4.md` 中的 Node.js 26 后端、Capacitor 手机壳、纯外部 CLI 托管总主会话模式，以及“点取消即冻结整个会话”的旧交互。
- 仓库归档策略：本规范面向全栈 Rust 演进。进入阶段 1A 实施时，先将现有 Node 26 实现移入 `legacy-node/` 归档对照（用于逐条迁移现有的 133 个幂等、竞态与崩溃恢复边界测试），保留 `docs/superpowers/research/` 与 `DESIGN.md` 资产；阶段 1A 验收通过后移除 `legacy-node/`（2026-11-26 用户决定）。

---

## 1. 文档定位与演进背景

Polyclave 的核心使命始终不变：**在本机运行的、绝对私有可控的多厂商 Agent 中枢控制台**——不把代码、会话和推送内容交给第三方云控制面，不照信子智能体自报的“我改完了”，由宿主根据外部 Git/远端状态硬核证结果，由总主会话中枢统一汇总上下文并调度各家最擅长的编码与评审 Agent。

### 1.1 从 v4 继承的核心原则（不变）

1. **R1 — 推送不等于完成**：只有当本轮唯一有效令牌签收了唯一的 `submit_result`、远端/宿主裸镜像目标分支 tip 与签收 SHA 一致、`base_sha` 是其祖先、区间提交非空且全区间每个提交均携带 `Run-Id: <runId>` git trailer 时，任务才算 `done`。
2. **四步启动握手**：先落库 `prepared` 轮次，启动受控包装器并将操作系统进程标识落库更新为 `authorized` 后，才向控制管道发送 `go` 放行；`go` 之前崩溃 100% 判定为 `never_started`。
3. **每轮独立一次性 MCP 令牌与幂等结算**：每个任务轮次签发仅在当前轮次有效的 MCP Bearer Token；相同 `(session_id, request_id)` 幂等受理，相同 `submit_result` 幂等返回，不一致则拒收。
4. **物理隔离用户全局 Agent 配置（同时持久保留隔离原生记忆）**：启动外部 CLI 时使用私有隔离目录 `<state-dir>/isolated-homes/<vendor>/`，仅通过系统链接或受控同步共享登录凭据与 `skills`（沿用 `cc-switch` 习惯），屏蔽用户全局杂乱 MCP 与自定义钩子，严禁把模型 API Key 或 GitHub 写权限 Token 注入不该持有的进程。各家 Agent 在 `<state-dir>/isolated-homes/<vendor>/` 下积累的对话历史与原生记忆**跨任务持久保留，绝不随任务完成删除**；仅在轮次结束/取消/恢复时立刻清理 `<state-dir>/tmp/` 下已作废的一次性 `0600` 临时 MCP 令牌配置文件。
5. **信任模型与视觉语言**：安全边界防“Agent 幻觉、越界修改、误操作、本机任意网页跨站请求和间接提示词注入”，不防同机恶意用户态提权；桌面端定稿采用统一三栏的深空黑曜（Deep Obsidian）/ 白昼冷晶（Daylight Crystal）光学视觉体系、七彩棱镜输入框、顺时针七彩像素棱镜 Logo、四宠方砖像素伙伴、右栏四标签拓扑核查工作台及底栏双环 / Settings 用量仪表盘（源稿见 `docs/superpowers/design/polyclave-v5-desktop-ui.fig`；`DESIGN.md` 将于阶段 1B `apps/polyclave-app/ui` 落地时同步更新）。
6. **双层记忆与分权防污染**：在持久保留外部 Agent 原生记忆与 Prompt Cache 的同时，于 `polyclave.sqlite` 内置纯本地跨项目 SQLite FTS5 记忆库，经宿主 HTTP MCP 统一开放给所有厂商；宿主代码自动固化客观硬事实，下层 Worker 与顾问 `manager` 仅可提案 `candidate` 候选记忆，由主控 `hub` 与用户审核晋升为 `verified` 并通过 `supersede` 版本链演进，杜绝低可信输出污染全局决策与红线。

### 1.2 横向吸收自外部项目的工程设计

结合 `OrchestratorInc/agent-orchestrator`（AO）、`stablyai/orca`（Orca）、`pingdotgg/t3code`（T3 Code）与 `deepseek-ai/dsh-superagent-prototype`（DSH Superagent）的架构对比与前期实测（见 `docs/superpowers/research/`），Polyclave v5 明确采纳与改造如下设计：

| 来源项目 | 采纳进 Polyclave v5 的设计 | Polyclave v5 的差异化改造 |
|---|---|---|
| **Agent Orchestrator (`ao`)** | 1. **后台守护进程与薄前端彻底分离**：`polyclaved` 独立持有状态、数据库与子进程树，关闭窗口任务不断。<br>2. **SQLite `change_log` + SSE `Last-Event-ID`**：每次事务同步记录增量序号，桌面休眠唤醒或手机经 Tailscale 重连时无损补齐状态。<br>3. **后台 `SCM Observer`**：轮询 GitHub PR/CI 状态与评审评论。<br>4. **世代分支隔离（Attempt 分支）**：每次运行尝试使用独立分支 `agent/<runId>/a<n>`。 | 1. 拒绝 AO “CI 失败直接自动打回 Worker”的黑盒回路，改为把 PR/CI 事件推回**总主会话中枢**，由中枢带全局上下文与配额预算决定重派、换模型还是询问用户。<br>2. 用 **Attempt 世代分支 + Windows Job Object / macOS 匿名管道守卫**取代 v4 “取消即冻结会话”的苛刻机制。<br>3. 零云控制面（无 `contracts/cloud`）、零遥测。 |
| **Orca (`stablyai/orca`)** | 1. **原生流式协议与会话续接**：Worker 侧优先走原生流式 JSON 输出与显式会话续接，结合阶段 2+ 长驻协议演进（`codex app-server`、`agy`/`dsh` ACP）。<br>2. **扫码一次性票据换长期凭证**的设备配对机制与宿主端撤销。<br>3. **基于启动时间戳确认进程身份**，拒绝纯 PID 复用误判。 | 阶段 1A Worker 采用**每轮有界流式 CLI 进程**（Codex `exec --json` / `exec resume`；Claude `-p --output-format stream-json --session-id` / `--resume`），确保每轮强制轮换一次性 `mcp_token` 并严格通过四步启动握手；`codex app-server` 与 ACP 留待阶段 2+ 评估。绝不采信 Worker 自报完成（必须过 R1 外部核证）；绝不镜像或修改用户的全局 `~/.codex` / `~/.claude` 配置；推送绝不经第三方网关明文转发。 |
| **T3 Code (`t3code`)** | 1. **宏流程步骤级 Checkpoint 重放**：已成功步骤落盘输入哈希与输出，宿主重启不重放外部副作用。<br>2. **Reviewer 强制使用一次性 Worktree**（已实测证实 `plan` 模式锁不住 Codex/AGY）。<br>3. **结构化输出最多 2 次自动纠偏**，超限转人工或失败分支。 | 不引入庞大的通用脚本引擎，只保留 **`council`（多模型盲评方案）** 与 **`pr_gate`（范围检查与 SHA 绑定合并门禁）** 两个宿主内置强类型原子宏流程，其余灵活编排归总主会话中枢。 |
| **`dsh-superagent-prototype` (`deepseek-ai`)** | 1. **主动值守管家心智（Proactive Personal Agent Bot）**：以“最小化人类注意力消耗（Attention Budget）”为目标，从被动一问一答升级为事件驱动的主动项目管家。<br>2. **准入策略矩阵（`Admission Policy`）**：宿主代码层严格区分本人客户端显式指令（`explicit`）与外部仓库/系统事件推断候选（`inferred`：`off \| observe \| auto`），并对外部机器人账号消息做防自激隔离。<br>3. **结构化 `Ticket` 与 `Mandate`（任务单与授权契约）**：将获准需求固化为带明确范围边界和派活预算（`max_runs`）的工单，统领其下多轮跨厂商子智能体 `Run`，防止对话跑偏或提示词注入失控。 | 1. **摆脱单仓库与 DSH Bundle 绑定**：升级为跨多项目仓库、可调度多厂商 Worker（Codex / Claude / AGY / DSH）的独立全栈 Rust 中枢守护进程。<br>2. **100% 私有通信边界（拒绝第三方商业 IM 云）**：不接入飞书、Telegram 等第三方 IM 平台，客户端统一使用自研 Tauri v2 + 轻量原生 Web/TS（经 Tailscale 端到端加密直连），私有通知仅允许本机或运行在 Tailscale 私有网内的开源自托管渠道（如自建 `ntfy` / Matrix），确保连每一句对话文字也绝不经过第三方云端（2026-11-26 用户决定）。 |

---

## 2. 核心用户决策汇总

以下均经 2026-11-26 架构评审与审查修订后最终确认：

1. **后端全栈 Rust 重写与双平台原生支持**（2026-11-26 用户决定）：追求原生执行效率与无外部运行时依赖，放弃 Node 26 后端，在当前仓库演进重写为纯 Rust 后端工作区（Cargo Workspace）；宿主原生支持 **macOS 与 Windows**（同时兼容 Linux）。
2. **客户端采用 Tauri v2（Rust 外壳）+ 轻量原生 Web/TypeScript 界面**（2026-11-26 用户决定）：利用 Rust 原生 Tauri v2 负责多端原生能力（守护进程自动拉起、系统托盘、系统钥匙串、原生通知与桌面/移动打包）；界面层沿用并演进现有的零框架 HTML/CSS/`DESIGN.md` 资产与轻量 TypeScript，通过 `polyclave-proto` 的 `ts-rs` 在 `cargo test` 时自动导出 `.d.ts` 契约保障前后端类型对齐。不使用重型浏览器 WASM 框架，从而消除跨边界 DOM 拷贝开销与数 MB 包体延迟，确保 10–20ms 瞬开、60/120fps 原生 `Web Animations API` 丝滑动效，并天然支持由 `polyclaved` 直接静态托管供 Tailscale 浏览器免安装访问。
3. **守护进程与薄客户端解耦（方案 A）**（2026-11-26 用户决定）：后端守护进程 `polyclaved` 独立监听 `127.0.0.1:4310`（HTTP + SSE）；桌面 Tauri 客户端启动时自动探测并按需拉起本地 `polyclaved`，关闭窗口不中断后台运行的 Agent；移动端与远端电脑经 Tailscale 访问同一套受 Token 保护的 HTTP + SSE 接口。
4. **主控会话本地主权原则（Canonical Hub Ledger）**（2026-11-26 用户决定）：
   - **主控会话（`role = hub`）永远留在本项目的 `polyclave.sqlite` 中**，作为项目开发进度、架构讨论、设计决策与上下文摘要的唯一权威真相源。
   - 主控会话允许切换两类模型驱动方式（实现统一的 `HubModelProvider` trait）：
     - **原生 API 接入（如通过 DeepSeek API 调用 `deepseek-v4.1`）**：由 `polyclaved` 直接发起流式 HTTPS API 请求，保存思维链与工具调用记录。
     - **借用 `codex` 或 `claude` CLI 作为主控模型提供方（增量续接 + 双写归档）**：允许利用本地已登录的 Codex 或 Claude Code CLI 执行主控推理，在 `isolated-homes/<vendor>/` 下持久保留各家 CLI 的原生线程与记忆，通过 `hub_provider_threads` 表记录 `(session_id, profile_id, agent_thread_id, last_synced_msg_id)`：连续使用同一 CLI 时直接 `--resume` 仅发送增量消息（吃满原生记忆与 Prompt Cache）；跨模型/API 切回原 CLI 时继续 `--resume` 并只补齐其缺席期间的差量消息；同时**绝不让外部 Agent 独享主控状态**，每轮结束后把 CLI 的回复全文、推理摘要与调度意图完整双写归档回 `polyclave.sqlite` 的 `messages` 表（一旦外部 CLI 线程损坏或丢失，宿主随时基于 `rolling_summary` 与近期 `messages` 无损重建）。
5. **中枢集中调度（信息流开放、控制流收口）**（2026-11-26 用户决定）：平时子智能体完成后，宿主核证结果并将交付摘要自动回传给主控会话，由主控会话在 `Ticket` 预算（`max_runs`）内决定下一步派谁（前序产物路径、SHA 与摘要通过 `context_refs` 传给后续智能体）；除 `council` 盲评阶段外不强制隔绝信息，但子智能体绝不相互私自派活。
6. **主控升级为“主动值守的个人管家 Bot（Personal Superagent）” + 纯私有 Tailscale 触达**（2026-11-26 用户决定）：主控不仅是被动聊天框，而是具备统一事件收件箱（Event Triage Loop）、准入策略（`Admission Policy`）与 `Ticket / Mandate` 预算控制的主动大脑中枢；所有交互与通知严格限定在自研 Tauri 客户端与 Tailscale 私有网自托管开源通知渠道内，不依赖任何第三方聊天软件。
7. **三类可直聊会话角色（`hub` / `manager` / `worker`）与全写操作 Worktree 隔离**（2026-11-26 用户决定）：保留主控管家（`hub`）、各家常驻顾问（`manager`）和任务执行会话（`worker`）三类均可直接对话的会话。`hub` 与 `manager` 严格只读看仓，任何文件修改一律隔离在 `worker` 的独立 Attempt Worktree 中，彻底取消直接在主仓库 `repo_cwd` 原位改文件的无保护聊天模式，从架构根源消除主工作区并发写坏风险。
8. **稳健里程碑切分（阶段 1A → 阶段 1B）**（2026-11-26 用户决定）：先交付 **阶段 1A（`polyclave-core` 执行底座 + 双平台进程/存储/本地裸镜像 R1/Worker 闭环 + `memory_items` 底座与基础记忆 MCP）** 并跑通双平台测试与真实 CLI 探针门禁、淘汰 `legacy-node/`；再交付 **阶段 1B（`polyclave-hub` 主动管家主控引擎、`hub_provider_threads` 增量续接、记忆晋升策展、常驻顾问会话 + `polyclave-app` Tauri v2 轻量 TS 桌面端）**。
9. **双层记忆系统与外部 Agent 原生记忆留存**（2026-11-26 用户决定）：
   - 持久保留 `<state-dir>/isolated-homes/<vendor>/` 下各家 Agent 的历史对话与原生记忆（不随任务完成清理），仅清理 `<state-dir>/tmp/` 下已失效的一次性 `0600` 临时 MCP 令牌文件；
   - 在 `polyclave.sqlite` 内置纯 Rust + SQLite FTS5 的跨项目公共记忆系统（覆盖 `global` / `project` / `run` 三层作用域），经宿主 HTTP MCP 开放给全部厂商与角色接入，实行分权写入（宿主自动固化 / Worker 提案 `candidate` / 主控与用户晋升 `verified` 与 `supersede`）以防范幻觉与间接提示词注入。

---

## 3. Cargo Workspace 模块架构与边界

整个工程组织为单个 Rust Cargo Workspace + 轻量前端子目录，严格约束依赖单向流动：

```text
polyclave/
├── Cargo.toml                        # Workspace 清单与统一依赖版本锁
├── crates/
│   ├── polyclave-proto/              # 纯数据契约与状态枚举 (serde, ts-rs)，零 OS/IO 依赖，自动生成 proto.ts
│   ├── polyclave-os/                 # 跨平台抽象：进程组/Win32 Job Object、启动握手包装器、路径规范化、Junction/同步
│   ├── polyclave-store/              # SQLite 存储层 (rusqlite bundled+fts5)、单写者通道、WAL 事务、唯一约束、change_log、记忆库
│   ├── polyclave-adapters/           # 下层执行器适配器 (Codex / Claude / AGY / DSH) + 宿主 HTTP MCP Server（含基础记忆工具）
│   ├── polyclave-core/               # 阶段 1A 核心执行底座：MirrorManager、WorktreeManager、TurnRunner、RunManager、classify_run、Recovery
│   └── polyclave-hub/                # 阶段 1B+ 中枢大脑（依赖 polyclave-core）：HubModelProvider、只读看仓工具、Ticket 分诊、记忆策展、宏流程与 Observer
├── bins/
│   └── polyclaved/                   # 守护进程二进制 (axum HTTP/SSE、Token 鉴权、静态托管、--supervisor-exec 入口)
├── apps/
│   └── polyclave-app/
│       ├── src-tauri/                # Tauri v2 Rust 原生外壳：守护进程拉起、系统托盘、系统钥匙串 (keyring)、原生通知
│       └── ui/                       # 轻量 TypeScript + DESIGN.md CSS 界面（由 Tauri 打包，也可由 polyclaved 直接托管）
│           └── src/generated/proto.ts # 由 polyclave-proto (ts-rs) 在 cargo test 时自动生成
└── legacy-node/                      # 阶段 1A 实施期临时归档的旧 Node 26 实现与测试对照（1A 验收后删除）
```

### 各单元核心职责与边界

- **`polyclave-proto`**：定义 `SessionDto`、`MessageDto`、`TicketDto`、`RunDto`、`AttemptDto`、`TurnDto`、`MemoryItemDto`、`ProviderProfileDto`、`SseEnvelope`、`ApiError` 等全部前后端传输类型，启用 `ts-rs` 导出强类型定义至 `apps/polyclave-app/ui/src/generated/proto.ts`。其中 `TurnDto` **在编译期物理剔除 `mcp_token` 字段**，确保 REST、SSE 与 `change_log` 永不泄露轮次令牌。
- **`polyclave-os`**：屏蔽 macOS 与 Windows 的系统差异，暴露纯正平台原语：`spawn_supervised`、`process_identity`、`boot_epoch`、`kill_process_tree`、`canonical_real_path`、`resolve_cli_binary`、`prepare_isolated_link`。上层业务代码严禁出现零散系统条件编译分支。
- **`polyclave-store`**：唯一持有 `rusqlite::Connection` 的 crate。启用 `rusqlite` `bundled` 与 `fts5` 特性，采用“单写者专属线程/通道 + 读连接池（`PRAGMA journal_mode = WAL`, `PRAGMA busy_timeout = 5000`）”，消除异步 handler 持锁阻塞与写争用；所有状态跃迁在 `BEGIN IMMEDIATE` 事务内完成，并将 `SQLITE_CONSTRAINT_UNIQUE`（`2067`）映射为领域错误（`BusyError` / `ConflictError` / `BudgetExceededError`）；托管 `hub_provider_threads`、`memory_items` 与 `memory_items_fts` 全文检索表。
- **`polyclave-adapters`**：封装各家 CLI 的参数构建、事件流解析（转为统一 `AgentEvent`）与宿主内置 HTTP MCP Server（阶段 1A 暴露 `submit_result`、`report_progress`、`consult`、`memory_brief`、`memory_search`、`memory_propose`）。
- **`polyclave-core`（阶段 1A 核心执行引擎）**：承担不依赖上层模型大脑的确定性宿主底座能力——`MirrorManager`（宿主托管只追加本地裸镜像 + 每仓库异步互斥锁）、`WorktreeManager`、`TurnRunner`（四步启动握手与取消）、`RunManager`（Attempt 世代管理、`classify_run` 外部核证；手动 `POST /api/runs` 缺省 `ticket_id` 时在同一事务自动创建单 Run `explicit_user` 工单与专属 `worker` 会话）、`Recovery` 启动恢复，以及在 `Run` 进入 `done` 或人工 `resolve` 时自动向 `memory_items` 写入 `verified` 事实记录。
- **`polyclave-hub`（阶段 1B+ 中枢大脑，单向依赖 `polyclave-core`）**：管理 `hub` / `manager` 会话的 `HubModelProvider`（原生 API 流式推理 + 基于 `hub_provider_threads` 的 CLI 增量续接与双写归档）、只读看仓工具、`Admission / Ticket` 预算分诊循环、`<project_memory_brief>` 静态前缀注入、`memory_commit` / `memory_supersede` / `memory_archive` 记忆策展，以及阶段 2 的 `council` / `pr_gate` 宏流程与 `SCM Observer`。
- **`polyclaved`**：无头后台服务进程；`fn main()` 最顶部首先检查是否携带 `--supervisor-exec`，若是则不初始化 Tokio/SQLite，直接进入纯同步轻量包装器逻辑。
- **`polyclave-app`**：薄客户端。`src-tauri` 负责拉起 `polyclaved`、读写本地 `local-auth.token` 与系统钥匙串、系统托盘与原生通知；`ui` 为轻量静态 TS/HTML/CSS，支持 Tauri 容器内运行与浏览器免装访问。

---

## 4. 阶段 1A 与 1B 详细技术规范

### 阶段划分与验收边界

- **阶段 1A（Rust 守护进程核心底座 + 双平台等价交付）**：
  - 交付 `polyclave-proto`、`polyclave-os`、`polyclave-store`（含 `memory_items` + FTS5 检索与 `hub_provider_threads` 表结构）、`polyclave-adapters`（Codex + Claude 每轮流式 Worker 适配 + 记忆 MCP 工具）、`polyclave-core`（宿主本地裸镜像、每仓库互斥锁、Worktree、R1 核证与崩溃恢复）、`polyclaved`（HTTP/SSE + 本机 Token 鉴权与浏览器 `#token=` 引导）；持久保留 `isolated-homes/<vendor>/` 原生记忆，清理 `<state-dir>/tmp/` 临时 MCP 令牌文件。
  - 同步更新 `scripts/privacy-scan.sh`（对 `Cargo.lock` 豁免 400 KB 体积上限，保留路径与密钥正则扫描）与开发协作文档命令；迁移并跑通 `legacy-node/tests/` 全部核心幂等、竞态、启动握手、Worktree/R1 核证与崩溃恢复不变量测试，以及 GitHub Actions `macos-latest` + `windows-latest` 双平台 CI（含 `polyclave-proto` `ts-rs` 导出一致性检验）；完成真实 CLI 探针门禁（`POLYCLAVE_REAL_CODEX=1`、`POLYCLAVE_REAL_CLAUDE=1`）后移除 `legacy-node/`。
- **阶段 1B（主动管家主控引擎 + 常驻顾问 + Tauri v2 轻量 TS 桌面端）**：
  - 交付 `polyclave-hub`（`ApiHubProvider` 支持 DeepSeek `deepseek-v4.1` 等 + `CliHubProvider` 基于 `hub_provider_threads` 增量续接回写 + 只读看仓工具 + `Admission / Ticket` 预算分诊循环 + `<project_memory_brief>` 注入与 `memory_commit` / `memory_supersede` 策展 + `manager` 常驻顾问会话）。
  - 交付 `apps/polyclave-app`（Tauri v2 原生外壳 + 沿用 `DESIGN.md` 与 `Web Animations API` 动效的轻量 TS 界面，含跨项目与单项目「记忆 Memory」管理视图，启用 UI `tsc --noEmit` CI 门禁），并在无头浏览器（1440px 与 375px）完成全部 UI/交互与动画验证。

---

### 4.1 状态目录与 SQLite Schema（`polyclave-store`，闭合全部 P0 约束）

#### 状态目录布局

默认路径为 macOS `~/.polyclave`、Windows `%USERPROFILE%\.polyclave`（可通过 `--state-dir` 或 `POLYCLAVE_STATE_DIR` 覆盖）：

- `polyclave.sqlite`（及 `-wal` / `-shm`）：唯一持久化状态库。
- `host.lock`：JSON 文件 `{ "pid": 12345, "started_at": "...", "instance_id": "...", "port": 4310 }`，原子排他创建/替换。
- `local-auth.token`：守护进程启动生成的 256-bit 随机本机客户端鉴权密钥，Unix 权限 `0600`，Windows 设置仅当前用户 SID 可读写 ACL。
- `mirrors/`：宿主为每个登记开发仓库托管的只追加本地裸镜像 `<repo-hash>.git`。
- `isolated-homes/`：按 `codex/`、`claude/`、`hub-cli/` 存放隔离运行配置目录，**跨任务持久保留各家 Agent 的对话线程与原生记忆，绝不在任务结束后删除**。
- `tmp/`（Unix `0700`，Windows 仅当前用户 ACL）：**仅存放每轮含明文一次性 `mcp_token` 的 `0600` 临时 MCP 配置文件（如 `mcp-turn-<id>.json`）**，严禁放入 `worktrees/`，轮次结束、取消及启动恢复时立即清除。
- `worktrees/`：按 `<runId>-a<attemptNo>/` 隔离每个任务尝试的工作区。

#### 核心表结构与不变量约束

```sql
-- 1. 模型提供方档案表（闭合 [P0-4]：Key 严格绑定固定 base_url，禁止会话级改写 URL 窃取密钥）
CREATE TABLE provider_profiles (
  id              TEXT PRIMARY KEY,              -- 例如 "deepseek-api" / "codex-sub" / "claude-sub"
  display_name    TEXT NOT NULL,
  kind            TEXT NOT NULL CHECK (kind IN ('openai_compat', 'anthropic_compat', 'codex_cli', 'claude_cli')),
  base_url        TEXT,                          -- 仅 API 类使用，修改 URL 时必须重新校验/录入钥匙串密钥
  keyring_account TEXT,                          -- 对应系统钥匙串条目名，仅存标识，绝不存明文 Key
  models_json     TEXT NOT NULL,                 -- 允许选择的模型白名单列表
  created_at      TEXT NOT NULL
);

-- 2. 会话表：覆盖主控管家(hub)、各家常驻顾问(manager)与任务执行会话(worker)
CREATE TABLE sessions (
  id              TEXT PRIMARY KEY,              -- s + 8 hex
  name            TEXT NOT NULL,
  role            TEXT NOT NULL CHECK (role IN ('hub', 'manager', 'worker')),
  vendor          TEXT,                          -- manager/worker 对应厂商：'codex' | 'claude' | ...
  repo_cwd        TEXT NOT NULL,                 -- 默认关联仓库路径（worker 的文件写入严格限在 attempt worktree）
  profile_id      TEXT NOT NULL REFERENCES provider_profiles(id),
  provider_model  TEXT NOT NULL,                 -- 例如 deepseek-v4.1 / gpt-5-codex / claude-sonnet-4
  rolling_summary TEXT NOT NULL DEFAULT '',      -- 本地滚动压缩摘要（供 API 模式及 CLI 线程重建时组装上下文）
  agent_thread_id TEXT,                          -- 用于 worker/manager 续接参考；hub 多提供方映射见 hub_provider_threads
  created_at      TEXT NOT NULL
);

-- 2a. 主控/顾问外部 CLI 增量线程映射表（保留各家原生记忆与 Prompt Cache，同时守住本地主权）
CREATE TABLE hub_provider_threads (
  session_id         TEXT NOT NULL REFERENCES sessions(id),
  profile_id         TEXT NOT NULL REFERENCES provider_profiles(id),
  agent_thread_id    TEXT NOT NULL,              -- 该厂商隔离目录内对应的持久外部会话/线程 ID
  last_synced_msg_id INTEGER NOT NULL DEFAULT 0, -- 已同步给该外部线程的最新 messages.id
  updated_at         TEXT NOT NULL,
  PRIMARY KEY (session_id, profile_id)
);

-- 3. 消息表：项目对话、思维链与中枢调度回执的唯一权威账本
CREATE TABLE messages (
  id              INTEGER PRIMARY KEY AUTOINCREMENT,
  session_id      TEXT NOT NULL REFERENCES sessions(id),
  turn_id         INTEGER REFERENCES turns(id),
  source          TEXT NOT NULL CHECK (source IN ('user', 'agent', 'host')),
  client_kind     TEXT CHECK (client_kind IN ('desktop', 'web', 'ios', 'android', 'cli')),
  provider_label  TEXT,                          -- 实际产出条目的提供方/模型，如 "deepseek-api/deepseek-v4.1"
  reasoning       TEXT,                          -- 模型思考过程（reasoning_content 或思考摘要）
  text            TEXT NOT NULL,
  metadata_json   TEXT,                          -- 工具调用摘要、Ticket/Run/SHA 卡片数据
  queued          INTEGER NOT NULL DEFAULT 0 CHECK (queued IN (0, 1)),
  created_at      TEXT NOT NULL
);

-- 4. 准入策略与工单契约表（闭合 [P0-2]：单条用户授权绑定有限预算，防无限链式注入）
CREATE TABLE admission_policies (
  repo_cwd        TEXT PRIMARY KEY,
  inferred_mode   TEXT NOT NULL DEFAULT 'observe' CHECK (inferred_mode IN ('off', 'observe', 'auto')),
  bot_actors_json TEXT NOT NULL DEFAULT '[]',    -- 外部机器人账号列表，严格禁止触发执行或终止工单
  updated_at      TEXT NOT NULL
);

CREATE TABLE tickets (
  id              TEXT PRIMARY KEY,              -- t + 8 hex
  hub_session_id  TEXT NOT NULL REFERENCES sessions(id),
  repo_cwd        TEXT NOT NULL,                 -- 锁定本工单允许派发的目标仓库
  title           TEXT NOT NULL,
  mandate_json    TEXT NOT NULL,                 -- { goal, allowed_paths, forbidden_actions, acceptance_criteria }
  source_kind     TEXT NOT NULL CHECK (source_kind IN ('explicit_user', 'inferred_event')),
  basis_msg_id    INTEGER UNIQUE REFERENCES messages(id), -- 一条用户消息至多开启一张显式工单
  max_runs        INTEGER NOT NULL DEFAULT 4 CHECK (max_runs BETWEEN 1 AND 20),
  runs_used       INTEGER NOT NULL DEFAULT 0 CHECK (runs_used >= 0),
  status          TEXT NOT NULL CHECK (status IN ('proposed', 'active', 'blocked_on_user', 'done', 'cancelled')),
  created_at      TEXT NOT NULL,
  updated_at      TEXT NOT NULL
);

-- 5. 任务运行表（闭合 [P0-5]：显式存储 repo_cwd、max_attempts 与 worker_session_id）
-- 注：阶段 1A 或手工 POST /api/runs 缺省 ticket_id 时，accept_run 在同一事务内自动创建一条
-- max_runs = 1 的 explicit_user 显式工单与专属 role='worker' 会话，保持模型统一。
CREATE TABLE runs (
  id                  TEXT PRIMARY KEY,          -- r + 8 hex
  ticket_id           TEXT NOT NULL REFERENCES tickets(id),
  hub_session_id      TEXT NOT NULL REFERENCES sessions(id),
  worker_session_id   TEXT NOT NULL UNIQUE REFERENCES sessions(id), -- 每个 Run 独占一个 role='worker' 会话用于直聊与插话
  request_id          TEXT NOT NULL,
  repo_cwd            TEXT NOT NULL,
  vendor              TEXT NOT NULL CHECK (vendor IN ('codex', 'claude')),
  model               TEXT,
  instructions        TEXT NOT NULL,
  context_refs_json   TEXT,                      -- 主控传入的前序 SHA、摘要与参考文件路径
  mirror_remote       TEXT NOT NULL,             -- 宿主托管的只追加本地裸镜像绝对路径
  base_branch         TEXT NOT NULL,
  base_sha            TEXT NOT NULL,
  status              TEXT NOT NULL CHECK (status IN ('pending', 'running', 'done', 'interrupted', 'failed', 'needs_review')),
  current_attempt     INTEGER NOT NULL DEFAULT 0,
  max_attempts        INTEGER NOT NULL DEFAULT 3,
  accepted_attempt_id INTEGER,
  detail              TEXT,
  result_sha          TEXT,
  observed_sha        TEXT,
  result_source       TEXT CHECK (result_source IN ('agent', 'user')),
  resolution          TEXT,
  created_at          TEXT NOT NULL,
  updated_at          TEXT NOT NULL
);
CREATE UNIQUE INDEX idx_runs_idempotent_req ON runs(hub_session_id, request_id);

-- 6. 尝试世代表：每次开始或重试生成独立分支 agent/<runId>/a<attempt_no>
CREATE TABLE attempts (
  id               INTEGER PRIMARY KEY AUTOINCREMENT,
  run_id           TEXT NOT NULL REFERENCES runs(id),
  attempt_no       INTEGER NOT NULL,
  branch           TEXT NOT NULL,                -- agent/<runId>/a1, a2, ...
  worktree_path    TEXT NOT NULL,
  agent_thread_id  TEXT,
  corrections      INTEGER NOT NULL DEFAULT 0,
  status           TEXT NOT NULL CHECK (status IN ('prepared', 'running', 'completed', 'cancelled', 'superseded', 'interrupted', 'failed')),
  created_at       TEXT NOT NULL,
  ended_at         TEXT,
  UNIQUE(run_id, attempt_no)
);

-- 7. 轮次表（闭合 [P0-1] 与 [P0-3]，并解决 consult 并发自锁与 worker_chat 绑定约束）
CREATE TABLE turns (
  id               INTEGER PRIMARY KEY AUTOINCREMENT,
  session_id       TEXT NOT NULL REFERENCES sessions(id),
  run_id           TEXT REFERENCES runs(id),
  attempt_id       INTEGER REFERENCES attempts(id),
  parent_turn_id   INTEGER REFERENCES turns(id), -- 仅 mode = 'consult' 使用，事务强制父轮次 mode != 'consult'（深度=1）
  mode             TEXT NOT NULL CHECK (mode IN ('hub', 'manager', 'worker_chat', 'start', 'correction', 'retry', 'consult')),
  status           TEXT NOT NULL CHECK (status IN ('running', 'completed', 'cancelled', 'interrupted', 'failed')),
  phase            TEXT NOT NULL CHECK (phase IN ('prepared', 'authorized')),
  host_instance_id TEXT NOT NULL,
  boot_epoch       TEXT NOT NULL,
  os_proc_id       TEXT,
  proc_started_at  TEXT,
  mcp_token        TEXT NOT NULL UNIQUE,         -- 48 hex，仅当前轮次 running+authorized 时有效
  exit_code        INTEGER,
  end_reason       TEXT CHECK (end_reason IN ('exit', 'cancel', 'host_lost', 'never_started', 'dispatch_closed')),
  writer_check     TEXT CHECK (writer_check IN ('excluded', 'possible')), -- 运行期间必须为 NULL，结束时强制显式落盘
  started_at       TEXT NOT NULL,
  ended_at         TEXT,
  CHECK (
    (mode IN ('worker_chat', 'start', 'correction', 'retry') AND run_id IS NOT NULL AND attempt_id IS NOT NULL)
    OR (mode IN ('hub', 'manager') AND run_id IS NULL)
    OR (mode = 'consult' AND parent_turn_id IS NOT NULL)
  )
);
-- 两把正交部分唯一锁（显式排除 mode = 'consult' 避免轮内跨厂商咨询触发自锁死）
CREATE UNIQUE INDEX idx_turns_running_session
  ON turns(session_id) WHERE status = 'running' AND run_id IS NULL AND mode != 'consult';
CREATE UNIQUE INDEX idx_turns_running_run
  ON turns(run_id) WHERE status = 'running' AND run_id IS NOT NULL AND mode != 'consult';

-- 8. 交付签收表（闭合 [P0-3]：按 attempt_id 唯一，避免旧世代卡死新尝试）
CREATE TABLE results (
  id               INTEGER PRIMARY KEY AUTOINCREMENT,
  run_id           TEXT NOT NULL REFERENCES runs(id),
  attempt_id       INTEGER NOT NULL REFERENCES attempts(id),
  turn_id          INTEGER NOT NULL REFERENCES turns(id),
  accepted         INTEGER NOT NULL CHECK (accepted IN (0, 1)),
  payload_json     TEXT NOT NULL,                -- { run_id, attempt, branch, sha, summary, open_questions }
  error            TEXT,
  created_at       TEXT NOT NULL
);
CREATE UNIQUE INDEX idx_results_one_accepted_per_attempt ON results(attempt_id) WHERE accepted = 1;

-- 9. 跨项目公共记忆表 + FTS5 全文检索（所有厂商经宿主 MCP 共享、分权防污染）
CREATE TABLE memory_items (
  id              TEXT PRIMARY KEY,              -- m + 8 hex
  scope           TEXT NOT NULL CHECK (scope IN ('global', 'project', 'run')),
  repo_cwd        TEXT,                          -- scope = 'global' 时可为 NULL；'project'/'run' 必填
  kind            TEXT NOT NULL CHECK (kind IN ('decision', 'constraint', 'progress', 'codebase_fact', 'pitfall', 'preference', 'receipt')),
  title           TEXT NOT NULL,
  content         TEXT NOT NULL,
  tags_json       TEXT NOT NULL DEFAULT '[]',
  evidence_json   TEXT,                          -- 溯源：{ run_id, sha, files, detail }
  status          TEXT NOT NULL CHECK (status IN ('candidate', 'verified', 'superseded', 'archived')),
  supersedes_id   TEXT REFERENCES memory_items(id),
  created_by      TEXT NOT NULL,                 -- 'user' | 'host' | 'hub' | 'codex' | 'claude' | ...
  source_run_id   TEXT REFERENCES runs(id),
  source_turn_id  INTEGER REFERENCES turns(id),
  embedding_blob  BLOB,                          -- 阶段 4 预留：本地语义混合检索向量
  created_at      TEXT NOT NULL,
  updated_at      TEXT NOT NULL,
  CHECK ((scope = 'global') OR (repo_cwd IS NOT NULL))
);
CREATE INDEX idx_memory_scope_repo_status
  ON memory_items(scope, repo_cwd, status, updated_at DESC);

CREATE VIRTUAL TABLE memory_items_fts USING fts5(
  title,
  content,
  tags_json,
  content='memory_items',
  content_rowid='rowid'
);
-- 注：配套 AFTER INSERT / UPDATE / DELETE 触发器在 BEGIN IMMEDIATE 事务内保持 memory_items_fts 实时同步。

-- 10. 原始事件流与增量通知表
CREATE TABLE events (
  id               INTEGER PRIMARY KEY AUTOINCREMENT,
  turn_id          INTEGER NOT NULL REFERENCES turns(id),
  event_kind       TEXT NOT NULL,
  payload_json     TEXT NOT NULL,
  created_at       TEXT NOT NULL
);

CREATE TABLE change_log (
  seq              INTEGER PRIMARY KEY AUTOINCREMENT,
  topic            TEXT NOT NULL,                -- 'message' | 'turn' | 'ticket' | 'run' | 'attempt' | 'memory' | 'hub_receipt' | 'inbox'
  entity_id        TEXT NOT NULL,
  payload_json     TEXT NOT NULL,
  created_at       TEXT NOT NULL
);
```

### 4.2 跨平台进程管控与文件系统适配（`polyclave-os`）

| 能力 | macOS（及类 Unix）实现 | Windows 实现 |
|---|---|---|
| **进程树硬隔离与防逃逸** | 创建独立进程组（`process_group(0)`）；`polyclaved --supervisor-exec` 在 `fn main()` 最顶部拦截并直入纯同步逻辑（不启 Tokio/SQLite）。父进程发完 `go\n` 后**必须保持控制管道写端常开直至轮次结束**；父进程强杀时内核关闭写端，包装器 0ms 感知 EOF 并执行 `SIGTERM` → 2s → `SIGKILL` 清空整组。 | 必须以 **`CREATE_SUSPENDED`** 挂起标志创建子进程 → 调用 `CreateJobObjectW`（设 `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`，**不开启** `BREAKAWAY_OK`）并 `AssignProcessToJobObject` 绑定 → 再 `ResumeThread` 放行，消除创建竞态逃逸窗。句柄关闭或主动 `TerminateJobObject` 时由内核原子清空整棵进程树。 |
| **进程身份防 PID 复用与开机标识** | 通过 `sysctl` / `proc_pidinfo`（辅以固定英文 UTC `ps`）读取精确进程启动时间；结合 `kern.boottime` 秒微秒数校验跨重启。 | 通过 `OpenProcess` + `GetProcessTimes` 读取 100ns 精度 `CreationTime`；**严禁使用 `SystemTime - GetTickCount64` 反推（防休眠/校时漂移）**，优先读取内核 `NtQuerySystemInformation(SystemBootEnvironmentInformation)` 的本次开机唯一 128-bit `BootIdentifier` GUID（完全免疫 NTP 校时、修改系统时间、时区与休眠），不支持时退回 `BootTime - BootTimeBias`，并在阶段 1A `windows-latest` CI 实测锁定。 |
| **启动四步握手** | 1. SQLite 写 `prepared`；2. 启动包装器回传 `ready` 与系统标识；3. SQLite 更新 `authorized`；4. 写入 `go\n`（并持续保活管道）。 | 完全一致的四步握手，经匿名句柄传递 `ready` 与 `go\n`，未放行前崩溃 100% 判定 `never_started`。 |
| **路径与沙箱根规范** | `dunce::canonicalize` 消除 `/tmp` 等符号链接前缀。 | `dunce::canonicalize` **剥离标准库 `\\?\C:\...` UNC 前缀**，避免 `git` 与 CLI 报错；`which` 解析 `.exe` / `.cmd` / `.bat`。 |
| **隔离目录链接与记忆留存** | `skills` 与 `auth.json` 使用标准 Unix symlink；`<state-dir>/isolated-homes/<vendor>/` 跨任务持久保留原生记忆与会话历史，仅 `<state-dir>/tmp/mcp-turn-<id>.json` 用完即删。 | `skills` 目录共享使用免提权 **NTFS Junction（`junction` crate）**；单文件凭据避免被 CLI 原子 rename 打断硬链接，改用目录级 Junction（若隔离边界允许）或轮次启动前/结束后受控同步副本；`<state-dir>/isolated-homes/<vendor>/` 持久保留，`<state-dir>/tmp/` 临时 MCP 令牌配置随轮次结束立即清理。 |

### 4.3 宿主托管本地裸镜像、Attempt 世代与崩溃恢复（闭合 `[P0-3]` / `[P0-5]`）

1. **宿主托管的只追加本地裸镜像（R1/R3 在真实项目上的统一落地）**：
   - 无论 `repo_cwd` 的上游 `origin` 是 GitHub HTTPS/SSH URL 还是本地路径，宿主都自动在 `<state-dir>/mirrors/<sha256(real_repo_cwd)[..16]>.git` 初始化并维护一个裸仓，强制设置 `receive.denyDeletes=true` 与 `receive.denyNonFastForwards=true`。
   - **不可变基线引用推送**：派发任务前，宿主在 `repo_cwd` 解析 `base_branch` 对应的 `base_sha`，统一执行 `git -C <repo_cwd> push <mirror_remote> <base_sha>:refs/bases/<runId>`（写入每个 `Run` 独占的不可变 `refs/bases/<runId>`，绝不往镜像推送易因用户本地 `rebase` / `amend` / `reset --hard` 触发非快进拒收的可变分支名）。Worker 在 `network_access=false` 沙箱下只能将提交推至 `<mirror_remote>` 的 `refs/heads/agent/<runId>/a<n>`。
   - **分支引用回拉与非回滚语义**：当某个 `Run` 经 R1 核证通过进入 `done` 后，宿主执行 `git -C <repo_cwd> fetch <mirror_remote> refs/heads/agent/<runId>/a<n>:refs/heads/polyclave/<runId>` 并校验本地分支等于 `result_sha`；若因用户在 `repo_cwd` 并发操作 git 导致回拉短暂失败，仅记录可重试警告并支持幂等重同步，**绝不回滚已生效的 `done` 状态**（阶段 2 再由宿主用独立 Bot Token 推送到 GitHub 开 PR）。
   - **进程内每仓库异步互斥锁**：`polyclave-core` 的 `MirrorManager / WorktreeManager` 按规范化 `real_repo_cwd` 维护进程内互斥锁，串行化所有针对主仓库 `.git` 元数据的 `worktree add/prune`、镜像基线推送与 `done` 后回拉操作，杜绝并发任务互相踩坏 `.git` 锁。
2. **三类会话的取消、直聊与 Attempt 世代边界**：
   - **`hub` 与 `manager` 会话**：运行在纯只读看仓工具下，任何时候点击 `Stop turn` 取消均**不冻结、不重演**，立即允许继续对话。
   - **`worker` 会话（隔离在独立 Worktree 内直聊与插话）**：
     - 约束与锁：`worker_chat` 轮次**必须同时绑定 `run_id` 与 `attempt_id`**（由 `idx_turns_running_run` 保证同一 `Run` 内执行与直聊严格串行）。仅允许对非终态 `Run`（`status IN ('running', 'interrupted', 'needs_review')`）发起 `worker_chat`；已进入 `done` 或 `failed` 终态的 `Run` 一律拒绝 `worker_chat`（返回 `409 Conflict`），如需新改动必须新建 `Run`，确保已核证 `result_sha` 与工作区不可变。
     - 空闲直聊：用户在 `worker` 空闲时发送消息，直接在该 Attempt 的 `worktree_path` 内续接执行。
     - 运行中取消（`Stop turn`）：在 Windows 上 Job Object 确保 `writer_check = 'excluded'`，可原地继续直聊或重试；在 macOS 上若轮次已 `authorized`（`writer_check = 'possible'`），则当前 Attempt 标记为 `cancelled/superseded`，新指令或重试自动切到 **`attempt_no + 1`**（分支 `agent/<runId>/a<n+1>`、独立工作树 `worktrees/<runId>-a<n+1>`）。
     - **新世代构造与语义升级细则**：（a）新开启的 `attempt_no + 1` Worktree **必须从 `run.base_sha`（或已合法落盘的 `refs/bases/<runId>`）干净创建**，严禁从可能被脱离组子进程续写的旧 `a<n>` 工作区拷贝；（b）每次创建新 Attempt 均递增 `runs.current_attempt` 并受 `runs.max_attempts`（默认 3）约束；（c）相对 `legacy-node` 133 个测试中“远端存在分支即禁止 `allow_retry`”的旧规，v5 明确规定：`POST /api/runs/:id/resolve { decision: 'allow_retry' }` **允许在旧世代分支 `agent/<runId>/a<k>` 仍存在于镜像时放行重试**，将 `a<k>` 标为 `superseded` 并在下次 `retry` 时分配新分支 `agent/<runId>/a<k+1>`（阶段 1A 迁移对照表须显式记录此语义升级）。
3. **修复 `check_run_branch` 与全区间 `commit_summary`**：
   - `check_run_branch` 在校验 `base_sha..tip` 期间，若发现某中间提交缺少 `Run-Id` trailer，返回 `BranchCheck::Present { sha: tip, problem: Some(...) }`——**始终返回真实分支 `tip`**（问题详情里标明违规提交哈希），彻底修复旧版返回中间提交 SHA 导致无法手动 `Accept` 的死循环。
   - `commit_summary` 在本地镜像上执行 `git diff --numstat base_sha..sha` 与 `git log --format=%H%x09%s base_sha..sha`，向用户与主控展示**整个任务分支相对于 `base_sha` 的完整累计改动文件与全部提交主题**。
4. **统一判定表（`classify_run`，仅针对当前有效 `current_attempt` 分支 `agent/<runId>/a<current>`）**：

| `writer_check` | 当前 Attempt `accepted` 记录 | 镜像 `agent/<runId>/a<current>` 分支检查 | `classify_run` 结论 | 下一步动作 |
|---|---|---|---|---|
| `possible`（macOS 同次开机强杀且无法排除脱逃写入） | 任意 | 任意 | `needs_review` | 标为待核查；保留旧 Attempt 分支，允许用户切至 `attempt + 1` 重试或手工核对后采纳 |
| `excluded` | 无 | 缺失 (`absent`) | `retryable` (`interrupted`) | 安全：可自动或由主控决定开启 `attempt + 1` |
| `excluded` | 无 | 存在 (`present`) | `needs_review` | 推送了但未签收，展示全区间 `commit_summary` 等待人工 `accept` / `fail` / `allow_retry` |
| `excluded` | 有 (`sha = S`) | 存在且 `tip == S` 且全区间合法 | **`done`** | 核证通过，同步 `polyclave/<runId>` 引用、自动写入 `verified` `progress`/`receipt` 记忆并回传主控 |
| `excluded` | 有 (`sha = S`) | 缺失 / `tip != S` / 缺 Trailer | `needs_review` | 状态不一致，禁止自动标完成，交 `Needs you` 裁决（人工 `resolve` 同样落盘一条 `verified` 记忆） |
| 任意 | 任意 | 镜像读取失败 (`error`) | `check_failed` (`interrupted`) | 记录错误详情待重试核查 |

（注：已被取代的旧世代 `a<k>` 若事后收到滞后推送，宿主仅写入事件日志留痕，不改变现行 Attempt 状态。）

### 4.4 主控与顾问引擎、内置记忆系统、只读看仓工具及 Worker 适配器（`polyclave-hub` & `polyclave-adapters`）

#### `HubModelProvider` 双模接口与增量续接上下文拼装

- **原生 API 模式（`ApiHubProvider`，首选默认）**：支持绑定在 `provider_profiles` 上的兼容端点（内置 DeepSeek `https://api.deepseek.com` / `deepseek-v4.1` 等），从系统钥匙串读 Key；流式解析 `reasoning_content` 与原生 Tool Call；上下文按 **`[系统角色指令 + <project_memory_brief>] → [rolling_summary] → [已固化历史轮次]`** 稳定前缀拼装，将动态 `Ticket / Run` 状态快照与本轮最新输入置于末尾。
- **借用 CLI 模式（`CliHubProvider`，增量续接 + 双写归档）**：
  - 在 `<state-dir>/isolated-homes/<vendor>/` 持久保留各家 CLI 的会话历史与原生记忆，并通过 `hub_provider_threads` 表维护 `(session_id, profile_id, agent_thread_id, last_synced_msg_id)`：
    - **同提供方连续对话**：直接 `--resume <agent_thread_id>` 仅喂入上次之后的增量消息，保留原生记忆并最大化命中厂商 Prompt Cache；
    - **跨提供方切换再切回**：继续 `--resume` 原 CLI 线程，仅把其缺席期间由其他 API/厂商产出的差量消息补发给它；
    - **本地权威双写与无损重建**：每轮结束必把 CLI 回复全文、思考摘要与工具意图写回 `polyclave.sqlite` 的 `messages` 表；若外部线程损坏或失效，宿主自动新开线程并以静态前缀 `[系统角色指令 + <project_memory_brief> + rolling_summary + 近期 messages]` 无损重建上下文。
  - **Claude CLI 精确参数与 `--bare` 避坑规则（来自 `claude-code-probe` 调研）**：
    - 严禁使用 `--permission-mode plan`（实测不可靠且会触发内置规划流程）；统一使用 `--tools ""` 剥离全部内置写/Shell 工具，并用 `--allowedTools` 仅放行宿主提供的只读/调度/记忆 MCP 工具：
      `claude -p --verbose --output-format stream-json --include-partial-messages --strict-mcp-config --mcp-config <state-dir>/tmp/mcp-turn-<id>.json --setting-sources "" --tools "" --allowedTools mcp__host__... --max-turns <n>`，配合 `CLAUDE_CONFIG_DIR` 隔离视图。
    - **禁止对订阅登录传 `--bare`**：Claude Code 的 `--bare` 会禁用 OAuth 订阅登录并强制要求 `ANTHROPIC_API_KEY`；仅在用户显式配置 API Key 模式时开启 `--bare`。
    - 宿主 HTTP MCP 配置模板（`<state-dir>/tmp/mcp-turn-<id>.json`，`0600`）：`{ "mcpServers": { "host": { "type": "http", "url": "http://127.0.0.1:<port>/mcp", "headers": { "Authorization": "Bearer <mcp_token>" } } } }`。

#### 内置跨项目记忆系统与全厂商 MCP 接入

为实现跨项目进度掌握与多厂商接力共享知识，且不引入任何外部向量服务或第三方云：

- **三层作用域**：`global`（所有登记项目的活跃总览、跨项目偏好与工程红线）、`project`（按 `repo_cwd` 隔离的架构决策 `decision`、约束红线 `constraint`、里程碑进度 `progress`、代码库事实 `codebase_fact` 与实测踩坑 `pitfall`）、`run`（任务交付 `receipt`、改动统计与遗留问题）。
- **拉取与开局自动推送**：
  - 全部角色（`hub` / `manager` / `worker` / `reviewer`）经宿主 HTTP MCP 接入 `memory_brief({ repo_cwd? })`（传仓库路径返回该项目的活体档案，缺省则返回全部进行中项目的全局进度总览）与 `memory_search({ scope?, repo_cwd?, query, kinds?, limit? })`（SQLite FTS5 全文 + 结构化过滤检索，带溯源 `source_run_id` / `sha`）。
  - 启动 `hub` / `manager` / `worker` 轮次时，宿主将受控长度（≤ 2,000 tokens，按 `constraint → decision → pitfall → progress` 排序）的 `<project_memory_brief>` 拼入静态前缀，确保任意新派入的模型第 1 秒即掌握项目设计与当前进度。
- **分权写入防幻觉与防注入**：
  - **宿主自动固化（`verified`）**：`Run` 过 R1 进入 `done`、用户完成 `resolve` 或 `Ticket` 结单时，宿主代码自动写入带真实 `result_sha` 溯源的 `progress` / `receipt` 记忆；
  - **下层 `worker` / `manager` 仅可提案（`memory_propose`）**：只能写入 `status = 'candidate'` 候选记录（强制绑定 `created_by`、`source_run_id`、`source_turn_id`），**绝无权限修改或覆盖既有 `verified` 决策与红线**；
  - **主控 `hub` 与用户审核晋升（`memory_commit` / `memory_supersede` / `memory_archive`）**：在讨论或验收后将 `candidate` 晋升为 `verified`；更新既有结论时必须走 `supersede`（以 `supersedes_id` 串起版本链），永不物理覆写，支持随时回滚。
- **阶段 2 隔离边界提示**：`council` 盲评期间仍可读取长期项目记忆但彼此屏蔽同期方案；`reviewer` 可读取 `project` 记忆中的红线约束，但必须启用新一次性工作区，绝不复用编写该代码的 `worker` 原生线程。

#### 主控与顾问专用受控只读看仓工具

为避免 `hub` 与 `manager` 会话成为“盲人”，宿主直接内置 4 个进程内受控只读工具（不经过外部 Shell）：
- `list_repo_files({ repo_cwd, dir?, glob? })`
- `read_repo_file({ repo_cwd, rel_path, start_line?, end_line? })`
- `search_repo({ repo_cwd, pattern, glob? })`
- `get_run_diff({ run_id })`（返回指定 `Run` 相对于 `base_sha` 的全区间改动文件与受控长度 Diff）

**安全边界**：所有路径经 `dunce::canonicalize` 强制校验落在已登记 `repo_cwd` 内，硬编码拒绝读取 `.env*`、私钥文件、`.git/config` 凭据与二进制大文件。

#### 主动管家事件分诊循环（Event Triage & Ticket 预算护栏）

- **三类入站事件**：（1）客户端本人 `source = 'user'` 显式指令（`explicit`）；（2）下层 `Run` 终态/阻塞回执（`worker_receipt`）；（3）仓库/系统信号（`inferred`，受 `admission_policies` 约束：`off` 丢弃、`observe` 仅起草 `proposed` 提案、`auto` 仅认白名单人工触发并严防外部 Bot 账号自激）。
- **闭环自动接力与 `Ticket` 配额硬限制（闭合 `[P0-2]`）**：
  - `create_or_update_ticket` 对显式用户消息绑定唯一 `basis_msg_id` 并锁死 `repo_cwd` 与 `max_runs`（默认 4）。
  - 当下层 `Run` 完成后自动唤醒 `hub` 会话继续推进同一张 `Ticket` 时，宿主在 `BEGIN IMMEDIATE` 事务中原子检查 `ticket.status == 'active'`、`ticket.repo_cwd == run.repo_cwd` 且 `runs_used < max_runs` 才允许执行 `dispatch_task` 并递增 `runs_used`。配额耗尽或试图越仓派活立即拒收并把 `Ticket` 转为 `blocked_on_user`，必须由你发新消息才能解锁继续。
- **Attention-Budget 汇报纪律**：常规单次纠偏或 `Ticket` 中间步静默更新卡片进度；仅当整张 `Ticket` 验收完毕、出现路径超界/路线分叉或进入 `needs_review` 时主动发出决策卡片。

#### 下层编码 Worker（Codex / Claude）

- **阶段 1A 统一采用每轮有界流式进程（保证轮次 Token 轮换与四步握手）**：
  - **Codex Worker**：每轮启动 `codex exec --json` / `codex exec resume` + `--ignore-user-config`，`CODEX_HOME = <state-dir>/isolated-homes/codex`（持久保留对话与原生记忆），`sandbox_mode = "workspace-write"`、`network_access = false`、`writable_roots = [worktree_path, git_common_dir, mirror_remote]`，通过环境变量传入每轮一次性 `POLYCLAVE_MCP_TOKEN`。
  - **Claude Code Worker**：每轮启动 `claude -p --verbose --output-format stream-json --include-partial-messages --session-id <uuid>`（同一 Attempt 内续接与直聊使用 `--resume <uuid>`），挂载 `--strict-mcp-config --mcp-config <state-dir>/tmp/mcp-turn-<id>.json --setting-sources ""`，使用在 `POLYCLAVE_REAL_CLAUDE=1` 实机门禁中核准的非交互工具放行参数（保留代码读写与 Shell 工具，**不传 `--tools ""`**；订阅登录模式下**不传 `--bare`**），`CLAUDE_CONFIG_DIR = <state-dir>/isolated-homes/claude`（持久保留项目上下文与自动记忆）。
  - **令牌隔离与垃圾清理**：每次 Worker 轮次结束、取消或守护进程重启恢复时，宿主必须**立即删除 `<state-dir>/tmp/mcp-turn-<id>.json`**；而 `<state-dir>/isolated-homes/` 下的原生对话历史与记忆永久留存。
- **Worker MCP 工具权限边界**：仅暴露 `submit_result`、`report_progress`、`consult`（单层只读，深度=1）、`memory_brief`、`memory_search`、`memory_propose`；**物理不提供 `dispatch_task`、`create_or_update_ticket`、`memory_commit`、`memory_supersede`**。

### 4.5 Loopback 安全鉴权、SSE 流控与 Tauri v2 + 轻量 TS 界面

#### 本机安全与 HTTP/SSE 协议

- **强本机鉴权与浏览器免装引导（闭合 `[P0-4]`）**：
  - `polyclaved` 启动写入 `0600` 的 `local-auth.token`；**所有 `/api/*` 路由（含 `/cancel`、`/retry` 与 `/api/events`）必须携带 `Authorization: Bearer <token>`**（严禁经 URL query 传 token），并校验 `Host` 与严格 `Origin` 白名单（`tauri://localhost`、`http://tauri.localhost`、`https://tauri.localhost`、`http://127.0.0.1:<port>`）。
  - **浏览器免装直连引导流**：`polyclaved` 启动时在 stdout 打印带 URL Fragment 的本地地址 `http://127.0.0.1:<port>/#token=<local_auth_token>`（`#` 后的 Fragment 不会随 HTTP 请求发给服务端，也不会落入访问日志）；前端启动读取 `location.hash` 后立即通过 `history.replaceState` 抹除地址栏并写入内存/`sessionStorage`，随后所有 `fetch`（含 SSE）统一附带 `Authorization: Bearer`；若以无 hash 的普通地址打开，页面弹出本机 Token 粘贴校验框。
  - REST 路由仅允许切换已登记的 `(profile_id, provider_model)`，彻底封堵改写 `api_base_url` 窃取 Key 的攻击面。
- **双通道流式事件与 `change_log` 裁剪**：
  - 持久化状态变更（`message`、`turn`、`ticket`、`run`、`attempt`、`memory`、`hub_receipt`、`inbox`）写入 `change_log`（保留最近 10,000 条；`TurnDto` 在编译期物理不含 `mcp_token`；当客户端携带的 `Last-Event-ID` 满足 `< min_retained_seq - 1` 或 `> max_seq`——例如重建/清空数据库后——服务端统一下发 `event: resync_required` 触发前端全量对齐）。
  - 高频流式 token delta 与思考增量仅走内存 `tokio::sync::broadcast`，不写入 SQLite 避免写放大。
  - 前端使用 **`fetch` + `ReadableStream` SSE 解析器**（而非不支持自定义鉴权头的原生 `EventSource`），按增量事件局部更新 DOM 并复用/演进防打断焦点保持机制（`withFocusPreserved`）。

#### Tauri v2 + 轻量 TS 界面分工

- **`src-tauri`**：自动拉起与保活 `polyclaved`；读取 `local-auth.token` 注入授权上下文；通过 `keyring` 管理 API Key（向前端仅回传 `configured: bool` 与掩码尾号）；提供系统托盘驻留与桌面原生通知。
- **`ui`**：继承并演进 `DESIGN.md`（阶段 1B 落地时同步更新），采用统一三栏的深空黑曜 / 白昼冷晶双主题（`276px / 1fr / 432px` + `34px` 底栏）、右栏四标签切换（`拓扑 / 核查 / 文件 / 记忆`）、底栏各厂商额度微组件与 `Settings · 用量与额度` 详情页、七彩棱镜输入框、四宠像素伙伴与物理阻尼动效；`<860px` 切换三标签响应式布局。所有契约类型直接导入 `ts-rs` 自动生成的 `proto.ts`，页面零外部网络请求。

#### 桌面端 UI、品牌 Logo 与光学动效定稿规范（源稿：`docs/superpowers/design/polyclave-v5-desktop-ui.fig`）

1. **品牌标识 · 竖立六角彩色像素棱镜钻石（`9×15`）**：顶部 `1px` 细白光束直入 `1-3-3-5-5-5-3-3-1` 全对称白心，外圈严格按 **红 → 橙 → 黄 → 绿 → 蓝 → 靛 → 紫** 顺时针无缝渐变且左右零重复色；统一用作左上角侧栏 Brand Logo、Host 核证徽标、桌面应用图标，以及输入框右顶角棱镜图腾（配对称微星光）。
2. **双主题统一三栏与克制光学色板**：
   - 默认 **深空黑曜（Deep Obsidian）**：左/中/右三栏与底栏统一 `#0D0D10`，凹槽区（拓扑画布、Diff 井、代码容器）为 `#08080B`，卡片 `#141418` 配 `1px` 顶缘高光与中性细描边 `#23252F`。取消整框彩色描边与实心塑料色块：运行/待核查活动卡片仅以左侧 `2.5px` 竖向光学灯条 + 极淡定向光晕（whisper wash）+ 半透玻璃胶囊（翡翠绿 `#10221A / #23523D / #58CF98`；珊瑚暖橙 `#241612 / #593224 / #F09B78`）提示；主操作按钮 `Accept <sha>` 采用磨砂银白胶囊 `#F2F3F7`（深曜石字 `#0E0F14` + 翡翠对勾）；已完成与归档态收束至静态雾霾蓝 `#8AB4F8` / 暗紫 `#9B72CB`。
   - 亮色 **白昼冷晶（Daylight Crystal）**：统一 `#F4F7FB` / `#E8EEF6` / `#FFFFFF` / `#0F172A`（并预留暖炭玄岩 `#141413` 与暖白画册 `#F6F4EE` 可选光学预设）。
3. **对话内嵌公式与代码芯片**：采用 **无边框 + 与 UI 同系中性淡灰底**（暗色 `#24262E` / 亮色 `#E1E7F0`），仅等宽字体着色——暗色 **淡蓝紫 `#9EB4FA`**、亮色 **淡橙黄 `#CF7E1F`**。
4. **七彩棱镜输入框、全局底部极光与 Agent 像素伙伴**：输入框上下配备 `1.5px` 全七彩折光边线、切工水晶 `Auto` 档位、迷你液位试管与七彩角向渐变停止/发送环；桌面全局底部配备跨三栏与半透底栏的 **Global Horizon Aurora 动态极光背景**（参考 `RMvdesi/obsidian-prism` 底部地平线极光着色器思路，运行/思考时 ~30fps 柔和流动，空闲 1.2s 自动降频至 `0fps` 休眠）；输入框顶缘栖息 4 只原创像素伙伴——**DeepSeek `Deepy`（无脚钴蓝鲸，配尾鳍与动态喷水）**，以及按 `16×10` 零黑描边、`1×2` 竖眼、4 细腿极简方砖语法绘制的 **Codex `银白小龙`（GPT Dragon）**、**Claude `Clawd`（暖陶橙）** 与 **Gemini `小紫猫`（支持叠叠乐）**，并与右顶角 **像素棱镜钻石** 同框响应运行/思考/待核查/休眠状态。
5. **右栏四标签与底栏 / Settings 用量体系**：右栏 `拓扑` 展示主控 → 子代理 DAG（含厂商/模型、Token · 花费、分段上下文条与低对比度光学连线）及底部快捷核查卡；底栏右下角以双环圆圈（外环 5 小时额度 / 内环周额度；无 5 小时限制厂商用实心圆）实时展示 **Claude（`#D97757`）、Codex（`#F2F2F5` / `#0F172A`）、Gemini（`#8E84F5`）、DeepSeek（`#4F8BFF`）**；`Settings · 用量与额度` 融合 Agent HUD 配额条（冷调钴蓝 `#5B89D6` / 冰川青 `#5FB2D4` / 鸢尾紫 `#8E7CE8` + 高水位暖橙 `#E8835C` 预警，DeepSeek 余额白字）、`dsh-cost-meter` 环图 / 24h DNA 条 / 分时段堆叠柱图 / 逐轮 Sparkline，以及 `512K` 高硼硅上下文液位试管（合规采集仅依赖 SQLite 本地台账 + 官方余额 API + Agent HUD 只读同步）。
6. **物理动效与净室合规边界**：UI 光学质感与动效体系参考 `RMvdesi/obsidian-prism`（四档二阶阻尼弹簧曲线、全局底部极光着色器、512K 高硼硅流体舱、`CSS.highlights` 零 DOM 包裹五级水墨流式淡入、`60–120fps → ~30fps → 0fps` 分频休眠调度、三态 `0fps` 语义余晖与 `prefers-reduced-motion` 自动降级）；全部前端代码与图形资产 **100% 净室原创实现**（不复制任何 GPL-3.0 第三方源码），并在阶段 1B `apps/polyclave-app/ui` 实施时同步更新 `DESIGN.md`。

### 4.6 测试、双平台 CI 与实机门禁

1. **纯 Rust 确定性离线测试夹具**：
   - `fake-agent`：支持脚本化轮次锁、文件/管道屏障、模拟带 `Run-Id` trailer 提交推送、`submit_result` 及故意脱逃的子进程写入测试（覆盖 macOS 与 Windows 双平台恢复场景）。
   - `fake-llm-server`：本地回环 `axum` 模拟流式 SSE、`reasoning_content` 与 `tool_calls`，离线验证 `ApiHubProvider` 与 `Ticket` 配额拦截。
   - **硬性纪律**：严禁固定 `sleep` 凑时序；阶段 1A 必须完整迁移 `legacy-node/tests/` 的核心不变量用例，并新增测试覆盖：`memory_items` FTS5 检索与 `candidate` 写隔离、轮内 `consult` 不触发并发唯一锁死锁、`worker_chat` 终态拒绝 `409`、以及 `refs/bases/<runId>` 不可变镜像推送与回拉重同步。
2. **真实 CLI 门禁（必须实跑关掉，不再悬空）**：
   - 在具备本机环境的机器上通过环境变量显式开启 `POLYCLAVE_REAL_CODEX=1` 与 `POLYCLAVE_REAL_CLAUDE=1` 完成实机验收（含核实 Claude Worker 保留文件/Shell 工具与 `isolated-homes/` 记忆持久化、临时 `mcp-turn-<id>.json` 自动清理）；并在阶段 1B 启动前用实机探针验证一次 `CliHubProvider`（`--tools ""` 只读借脑）的实际延迟与工具调用准确率。
3. **GitHub Actions 矩阵与仓库治理同步**：
   - 在 `macos-latest` 与 `windows-latest` 上并行执行：`cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo test --workspace`（含 `polyclave-proto` 的 `ts-rs` 导出一致性检查）与 `scripts/privacy-scan.sh`（阶段 1A 同步更新以豁免 `Cargo.lock` 的 400 KB 文件大小限制，保留本地路径与密钥扫描，并同步更新 `AGENTS.md` / `REVIEWER.md` 验证命令）。
   - 阶段 1B `apps/polyclave-app/ui` 落地后，在 CI 矩阵追加 UI TypeScript `tsc --noEmit` 与无头浏览器验证。

---

## 5. 后续子项目路线图概览（阶段 2～4）

- **阶段 2：多模型内阁 `council` + 真实 GitHub PR 门禁与 `SCM Observer`**
  - 接入 **Antigravity (`agy`)** 与 **DeepSeek Harness (`dsh`)** 适配器。
  - 实现 `council` 原子宏流程：主控发起方案讨论 → 各家 `manager` 并行独立输出结构化 `Proposal` → 代码剥离厂商标识呈递“方案甲/乙/丙” → `direction` 级交用户拍板、`detail` 级结合偏好决策。
  - 实现 `pr_gate` 与后台 `SCM Observer`：三层 `touched_paths` 范围契约校验；强制异构模型 Reviewer 在一次性 Worktree 中评审确切 `head_sha`（不复用编写者 Worker 原生线程）；Bot 账号与 Reviewer 账号物理隔离、将本地镜像通过 R1 的分支推送到 GitHub、显式传 `commit_id` 批准并先落台账再开 auto-merge；PR/CI 失败或评论变更实时作为事件推回主控收件箱。
- **阶段 3：Tauri v2 移动双端（iOS / Android）与 Tailscale 纯私有通知配对**
  - 编译打包 iOS 与 Android Tauri v2 客户端，适配真机 375px 触控交互。
  - 实现扫码 10 分钟一次性票据换长期设备凭证、`tailscale serve` 身份头双重验证与宿主端设备吊销。
  - 坚持纯私有触达边界（拒绝第三方 IM 云）：移动端访问走 Tailscale 端到端加密通道（原生 App + 宿主静态托管响应式 Web 备选）；远端提醒默认走**运行在用户本机或 Tailscale 私有网内的开源自托管通知渠道**（如自建 `ntfy` / UnifiedPush 或自建 Matrix 服务），推送载荷不含敏感明文正文，App 唤醒后经 Tailscale `Last-Event-ID` 增量拉取完整内容。
- **阶段 4：CUA 桌面互斥锁与基于记忆库的本地进化循环**
  - 实现整机单把 CUA 互斥锁（检测键鼠空闲、用户桌面随时一秒收回）。
  - 在既有 `memory_items` 底座上叠加本地语义混合检索（填充 `embedding_blob`）与定期蒸馏总结：基于 `polyclave.sqlite` 历史工单、返工次数、评审打回率与运行耗时，通过本地模型或 API 生成版本化、带 `supersedes_id` 一键回滚的路由表、记忆规则与 Role Brief 优化建议（严格禁止触碰宿主门禁代码与用户全局 `skills`）。
