# Polyclave v5 设计规范：全栈 Rust 守护进程 + Tauri v2 / Leptos 中枢控制台

- 日期：2026-11-26
- 状态：v5 正式设计草案（待用户审阅）
- 取代：`docs/superpowers/specs/2026-11-25-agent-console-design-v4.md` 中的 Node.js 26 + 原生静态页 + Capacitor 技术栈、纯外部 CLI 托管总主会话模式，以及“点取消即冻结整个会话”的旧交互。
- 仓库归档策略：本规范面向推倒重来的全栈 Rust 演进。进入子项目 1 实施阶段时，先将现有 Node 26 实现移入 `legacy-node/` 归档对照（用于逐条迁移现有的 133 个幂等、竞态与崩溃恢复边界测试），保留 `docs/superpowers/research/` 与 `DESIGN.md` 资产，待子项目 1 验收通过后移除 `legacy-node/`（2026-11-26 用户决定）。

---

## 1. 文档定位与演进背景

Polyclave 的核心使命始终不变：**在本机运行的、绝对私有可控的多厂商 Agent 中枢控制台**——不把代码、会话和推送内容交给第三方云控制面，不照信子智能体自报的“我改完了”，由宿主根据外部 Git/远端状态硬核证结果，由总主会话中枢统一汇总上下文并调度各家最擅长的编码与评审 Agent。

### 1.1 从 v4 继承的核心原则（不变）

1. **R1 — 推送不等于完成**：只有当本轮唯一有效令牌签收了唯一的 `submit_result`、远端目标分支 tip 与签收 SHA 一致、`base_sha` 是其祖先、区间提交非空且全区间每个提交均携带 `Run-Id: <runId>` git trailer 时，任务才算 `done`。
2. **四步启动握手**：先落库 `prepared` 轮次，启动受控包装器并将操作系统进程标识落库更新为 `authorized` 后，才向控制管道发送 `go` 放行；`go` 之前崩溃 100% 判定为 `never_started`。
3. **每轮独立一次性 MCP 令牌与幂等结算**：每个任务轮次签发仅在当前轮次有效的 MCP Bearer Token；相同 `(session_id, request_id)` 幂等受理，相同 `submit_result` 幂等返回，不一致则拒收。
4. **物理隔离用户全局 Agent 配置**：启动外部 CLI 时使用私有隔离目录，仅通过系统链接共享登录凭据与 `skills`（沿用 `cc-switch` 习惯），屏蔽用户全局杂乱 MCP 与自定义钩子，严禁把模型 API Key 或 GitHub 写权限 Token 注入不该持有的进程。
5. **信任模型与视觉语言**：安全边界防“Agent 幻觉、越界修改、误操作和间接提示词注入”，不防同机恶意用户态提权；客户端沿用 `DESIGN.md` 定义的黑银工业风与荧光绿/橙动态警示视觉体系。

### 1.2 横向吸收自外部项目的工程设计

结合 `OrchestratorInc/agent-orchestrator`（AO）、`stablyai/orca`（Orca）与 `pingdotgg/t3code`（T3 Code）的架构对比与前期实测（见 `docs/superpowers/research/`），Polyclave v5 明确采纳与改造如下设计：

| 来源项目 | 采纳进 Polyclave v5 的设计 | Polyclave v5 的差异化改造 |
|---|---|---|
| **Agent Orchestrator (`ao`)** | 1. **后台守护进程与薄前端彻底分离**：`polyclaved` 独立持有状态、数据库与子进程树，关闭窗口任务不断。<br>2. **SQLite `change_log` + SSE `Last-Event-ID`**：每次事务同步记录增量序号，桌面休眠唤醒或手机经 Tailscale 重连时无损补齐状态。<br>3. **后台 `SCM Observer`**：轮询 GitHub PR/CI 状态与评审评论。<br>4. **世代分支隔离（Attempt 分支）**：每次运行尝试使用独立分支 `agent/<runId>/a<n>`。 | 1. 拒绝 AO “CI 失败直接自动打回 Worker”的黑盒回路，改为把 PR/CI 事件推回**总主会话中枢**，由中枢带全局上下文决定重派、换模型还是询问用户。<br>2. 用 **Attempt 世代分支 + Windows Job Object / macOS 匿名管道守卫**取代 v4 “取消即冻结会话”的苛刻机制。<br>3. 零云控制面（无 `contracts/cloud`）、零遥测。 |
| **Orca (`stablyai/orca`)** | 1. **长驻协议优先**：Worker 侧优先走原生流式协议（Claude `stream-json`、Codex `app-server` 备选与 `exec`、AGY/DSH `acp`）。<br>2. **扫码一次性票据换长期凭证**的设备配对机制与宿主端撤销。<br>3. **基于启动时间戳确认进程身份**，拒绝纯 PID 复用误判。 | 绝不采信 Worker 自报完成（必须过 R1 外部核证）；绝不镜像或修改用户的全局 `~/.codex` / `~/.claude` 配置；推送绝不经第三方网关明文转发。 |
| **T3 Code (`t3code`)** | 1. **宏流程步骤级 Checkpoint 重放**：已成功步骤落盘输入哈希与输出，宿主重启不重放外部副作用。<br>2. **Reviewer 强制使用一次性 Worktree**（已实测证实 `plan` 模式锁不住 Codex/AGY）。<br>3. **结构化输出最多 2 次自动纠偏**，超限转人工或失败分支。 | 不引入庞大的通用脚本引擎，只保留 **`council`（多模型盲评方案）** 与 **`pr_gate`（范围检查与 SHA 绑定合并门禁）** 两个宿主内置强类型原子宏流程，其余灵活编排归总主会话中枢。 |
| **`dsh-superagent-prototype` (`deepseek-ai`)** | 1. **主动值守管家心智（Proactive Personal Agent Bot）**：以“最小化人类注意力消耗（Attention Budget）”为目标，从被动一问一答升级为事件驱动的主动项目管家。<br>2. **准入策略矩阵（`Admission Policy`）**：宿主代码层严格区分本人客户端显式指令（`explicit`）与外部仓库/系统事件推断候选（`inferred`：`off \| observe \| auto`），并对外部机器人账号消息做防自激隔离。<br>3. **结构化 `Ticket` 与 `Mandate`（任务单与授权契约）**：将获准需求固化为带明确范围边界（Mandate）的工单，统领其下多轮跨厂商子智能体 `Run`，防止多轮对话跑偏（Conversational Drift）。 | 1. **摆脱单仓库与 DSH Bundle 绑定**：升级为跨多项目仓库、可调度多厂商 Worker（Codex / Claude / AGY / DSH）的独立全栈 Rust 中枢守护进程。<br>2. **100% 私有通信边界（拒绝第三方商业 IM 云）**：不接入飞书、Telegram 等第三方 IM 平台，客户端统一使用自研 Tauri v2 + Leptos（经 Tailscale 端到端加密直连），私有通知仅允许本机或运行在 Tailscale 私有网内的开源自托管渠道（如自建 `ntfy` / Matrix），确保连每一句对话文字也绝不经过第三方云端（2026-11-26 用户决定）。 |

---

## 2. 核心用户决策汇总

以下均经 2026-11-26 架构评审确认：

1. **全栈 100% Rust 重写与双平台原生支持**（2026-11-26 用户决定）：追求原生执行效率与无外部运行时依赖，放弃 Node 26 后端与原生静态页，在当前仓库推倒重写为纯 Rust Cargo Workspace；宿主原生支持 **macOS 与 Windows**（同时兼容 Linux）。
2. **客户端采用 Tauri v2 + Leptos（Rust 编译为 WASM）**（2026-11-26 用户决定）：界面层 100% 使用 Rust 编写，复用 Tauri v2 官方成熟的跨四端插件体系（系统托盘、系统通知、钥匙串访问、Deep Link 与 macOS/Windows/iOS/Android 打包能力），彻底省去 Capacitor 胶水层与业务 JS/TS。
3. **守护进程与薄客户端解耦（方案 A）**（2026-11-26 用户决定）：后端守护进程 `polyclaved` 独立监听 `127.0.0.1:4310`（HTTP + SSE）；桌面 Tauri 客户端启动时自动探测并按需拉起本地 `polyclaved`，关闭窗口不中断后台运行的 Agent；移动端与远端电脑经 Tailscale 访问同一套 HTTP + SSE 接口。
4. **主控会话本地主权原则（Canonical Hub Ledger）**（2026-11-26 用户决定）：
   - **主控会话（`role = hub`）永远留在本项目的 `polyclave.sqlite` 中**，作为项目开发进度、架构讨论、设计决策与上下文摘要的唯一权威真相源。
   - 主控会话允许切换两类模型驱动方式（实现统一的 `HubModelProvider` trait）：
     - **原生 API 接入（如通过 DeepSeek API 调用 `deepseek-v4.1`）**：由 `polyclaved` 直接发起流式 HTTPS API 请求，保存思维链与工具调用记录。
     - **借用 `codex` 或 `claude` CLI 作为主控模型提供方**：允许利用本地已登录的 Codex 或 Claude Code CLI 执行主控推理，但**绝不让外部 Agent 接管主控会话的状态或记忆**；宿主每次从本地数据库组装规范上下文以只读无改写权限调用 CLI，并在每轮结束后**把 CLI 的回复全文、推理摘要与调度意图完整复制归档回 `polyclave.sqlite`**。用户可在任意轮次把主控模型从 DeepSeek API 切到 Claude CLI 再切回 Codex，项目历史永远完整留存。
5. **中枢集中调度（信息流开放、控制流收口）**（2026-11-26 用户决定）：平时子智能体完成后，宿主核证结果并将交付摘要自动回传给主控会话，由主控会话统一决定下一步派谁（前序产物路径、SHA 与摘要通过 `context_refs` 传给后续智能体）；除 `council` 盲评阶段外不强制隔绝信息，但子智能体绝不相互私自派活。日常调度走主控会话单步中枢流转，复杂固定环节可调用内置的 `council` 与 `pr_gate` 原子宏流程。
6. **主控升级为“主动值守的个人管家 Bot（Personal Superagent）” + 纯私有 Tailscale 触达**（2026-11-26 用户决定）：主控不仅是被动聊天框，而是具备统一事件收件箱（Event Triage Loop）、准入策略（`Admission Policy`）与 `Ticket / Mandate` 管理能力的主动大脑中枢；所有交互与通知严格限定在自研 Tauri 客户端与 Tailscale 私有网自托管开源通知渠道（如自建 `ntfy` / Matrix）内，不依赖任何第三方聊天软件。

---

## 3. Cargo Workspace 模块架构与边界

整个工程组织为单个 Rust Cargo Workspace，严格约束依赖单向流动（上层可依赖下层，下层不得反向依赖上层）：

```text
polyclave/
├── Cargo.toml                  # Workspace 清单与统一依赖版本锁
├── crates/
│   ├── polyclave-proto/        # 纯数据契约与状态枚举 (serde, chrono/time)，零 OS/IO 依赖，兼容 wasm32
│   ├── polyclave-os/           # 跨平台抽象：进程组/Win32 Job Object、启动握手包装器、路径规范化、Junction/硬链接
│   ├── polyclave-store/        # SQLite 存储层 (rusqlite bundled)、WAL 事务、唯一约束、change_log 增量序号表
│   ├── polyclave-adapters/     # 下层执行器适配器 (Codex / Claude / AGY / DSH) + 宿主 HTTP MCP Server
│   └── polyclave-hub/          # 中枢引擎：HubModelProvider (API & CLI 回写归档)、Run 生命周期、classify_run、宏流程与 Observer
├── bins/
│   └── polyclaved/             # 守护进程二进制 (axum HTTP + SSE 服务、host.lock、CLI 子命令、受控包装器入口)
├── apps/
│   └── polyclave-app/
│       ├── src-tauri/          # Tauri v2 原生外壳：守护进程拉起、系统托盘、系统钥匙串 (keyring)、桌面/移动原生通知
│       └── ui/                 # Leptos (CSR WASM) 跨端界面：仅依赖 polyclave-proto 与 gloo-net/wasm-bindgen
└── legacy-node/                # 子项目 1 实施期临时归档的旧 Node 26 实现与测试对照（验收后删除）
```

### 各单元核心职责与边界

- **`polyclave-proto`**：定义 `SessionDto`、`MessageDto`、`RunDto`、`AttemptDto`、`HubProviderConfig`、`SseEnvelope`、`ApiError` 等全部前后端传输类型，保证守护进程、Tauri 外壳和 WASM 界面在编译期 100% 类型对齐。
- **`polyclave-os`**：屏蔽 macOS 与 Windows 的系统差异。只暴露纯正的平台原语：`spawn_supervised`、`process_identity`、`boot_epoch`、`kill_process_tree`、`canonical_real_path`、`resolve_cli_binary`、`prepare_isolated_link`。上层业务代码严禁出现 `#[cfg(windows)]` 或 `#[cfg(target_os = "macos")]` 的零散分支。
- **`polyclave-store`**：唯一持有 `rusqlite::Connection` 的 crate。所有状态跃迁（受理运行、认领轮次、签收结果、创建新 Attempt、写增量变更）均在 `BEGIN IMMEDIATE` 事务内完成，并将数据库唯一约束冲突（`SQLITE_CONSTRAINT_UNIQUE` / `2067`）显式映射为领域错误（`BusyError` / `ConflictError`）。
- **`polyclave-adapters`**：封装各家 Worker CLI 的参数构建、事件流解析（转为统一 `AgentEvent`）与宿主内置 MCP Server（axum 子路由或独立回环端口，仅暴露 `submit_result`、`report_progress`、`consult`）。
- **`polyclave-hub`**：系统的中枢大脑。一方面管理主控会话的两类 `HubModelProvider`（原生 API 流式推理 + CLI 只读调用与回复复制归档），另一方面编排 `RunManager`、`classify_run` 外部核证、`Recovery` 启动恢复，以及子项目 2 的 `council` / `pr_gate` 宏流程与 `SCM Observer`。
- **`polyclaved`**：无头服务进程（同时内置 `--supervisor-exec` 隐藏子模式充当跨平台受控启动包装器，避免额外维护第二个独立可执行文件）。
- **`polyclave-app`**：薄客户端。`src-tauri` 只负责宿主生命线管理与原生能力桥接（绝不把 SQLite 或业务逻辑搬进 Tauri 进程）；`ui` 负责渲染界面并直连 `polyclaved` 的 HTTP/SSE 接口。

---

## 4. 子项目 1 详细技术规范（Rust 核心底座 + 双平台中枢闭环 + 桌面界面）

子项目 1 的交付目标是：**在 macOS 与 Windows 上同时跑通完整的本地中枢闭环——用户通过 Tauri + Leptos 界面与留在本项目 SQLite 中的主控会话对话（支持 DeepSeek API 直连及 Codex/Claude CLI 借用回写），主控会话调度 Codex 或 Claude Worker 在各自 Attempt Worktree 中完成编码并经 R1 核证后自动回传结果，全程具备进程树无残留收割、断线 `Last-Event-ID` 补传与强杀崩溃恢复能力。**

### 4.1 状态目录与 SQLite Schema（`polyclave-store`）

#### 状态目录布局

默认路径为 macOS `~/.polyclave`、Windows `%USERPROFILE%\.polyclave`（可通过 `--state-dir` 或环境变量 `POLYCLAVE_STATE_DIR` 覆盖）：

- `polyclave.sqlite`（以及 `-wal` / `-shm`）：唯一持久化状态库。
- `host.lock`：JSON 文件 `{ "pid": 12345, "started_at": "...", "instance_id": "...", "port": 4310 }`，使用原子排他创建/替换，防止两个 `polyclaved` 实例同时写入同一状态目录。
- `isolated-homes/`：按 `codex/`、`claude/`、`hub-cli/` 存放隔离运行配置目录。
- `worktrees/`：按 `<runId>-a<attemptNo>/` 隔离每个任务尝试的工作区。

#### 核心表结构与不变量约束

```sql
-- 1. 会话表：主控会话(hub)与普通工作会话(chat)均在此持久化
CREATE TABLE sessions (
  id              TEXT PRIMARY KEY,              -- s + 8 hex
  name            TEXT NOT NULL,
  role            TEXT NOT NULL CHECK (role IN ('hub', 'chat')),
  repo_cwd        TEXT NOT NULL,                 -- 规范化后的默认工作仓库路径
  provider_kind   TEXT NOT NULL CHECK (provider_kind IN ('api', 'codex_cli', 'claude_cli')),
  provider_model  TEXT NOT NULL,                 -- 例如 deepseek-v4.1 / gpt-5-codex / claude-sonnet-4
  api_base_url    TEXT,                          -- provider_kind = 'api' 时使用
  rolling_summary TEXT NOT NULL DEFAULT '',      -- 本地滚动压缩摘要，供长对话与切换模型/CLI 借用时组装上下文
  agent_thread_id TEXT,                          -- 仅 role = 'chat' 续接外部会话时使用；role = 'hub' 禁止依赖外部黑盒历史
  frozen_reason   TEXT,                          -- 仅在极少数异常冲突需用户显式介入时保留兼容，常规取消不置位
  frozen_at       TEXT,
  created_at      TEXT NOT NULL
);

-- 2. 消息表：项目对话、思维链与中枢调度回执的唯一权威账本
CREATE TABLE messages (
  id              INTEGER PRIMARY KEY AUTOINCREMENT,
  session_id      TEXT NOT NULL REFERENCES sessions(id),
  turn_id         INTEGER REFERENCES turns(id),
  source          TEXT NOT NULL CHECK (source IN ('user', 'agent', 'host')),
  client_kind     TEXT CHECK (client_kind IN ('desktop', 'web', 'ios', 'android', 'cli')),
  provider_label  TEXT,                          -- 记录实际产出该条消息的提供方/模型，如 "api/deepseek-v4.1" 或 "claude_cli/sonnet"
  reasoning       TEXT,                          -- 模型思考过程（reasoning_content 或提取的思考摘要）
  text            TEXT NOT NULL,
  metadata_json   TEXT,                          -- 工具调用摘要、引用 run_id/sha 卡片数据
  queued          INTEGER NOT NULL DEFAULT 0 CHECK (queued IN (0, 1)),
  created_at      TEXT NOT NULL
);

-- 3. 轮次表：记录每次向 API 或 CLI 发起的执行轮次与握手状态
CREATE TABLE turns (
  id               INTEGER PRIMARY KEY AUTOINCREMENT,
  session_id       TEXT NOT NULL REFERENCES sessions(id),
  run_id           TEXT REFERENCES runs(id),
  attempt_id       INTEGER REFERENCES attempts(id),
  mode             TEXT NOT NULL CHECK (mode IN ('hub', 'chat', 'start', 'correction', 'retry', 'consult')),
  status           TEXT NOT NULL CHECK (status IN ('running', 'completed', 'cancelled', 'interrupted', 'failed')),
  phase            TEXT NOT NULL CHECK (phase IN ('prepared', 'authorized')),
  host_instance_id TEXT NOT NULL,
  boot_epoch       TEXT NOT NULL,
  os_proc_id       TEXT,                         -- macOS: pgid；Windows: pid+job 标记
  proc_started_at  TEXT,                         -- 系统级进程创建时间戳，防 PID 复用误杀
  mcp_token        TEXT NOT NULL,                -- 48 hex，仅本轮 running+authorized 且归属当前实例时有效
  exit_code        INTEGER,
  end_reason       TEXT CHECK (end_reason IN ('exit', 'cancel', 'host_lost', 'never_started', 'dispatch_closed')),
  writer_check     TEXT NOT NULL DEFAULT 'excluded' CHECK (writer_check IN ('excluded', 'possible')),
  started_at       TEXT NOT NULL,
  ended_at         TEXT
);
-- 数据库强制并发锁：同一会话同一时刻至多 1 个 running 轮次
CREATE UNIQUE INDEX idx_turns_one_running_per_session
  ON turns(session_id) WHERE status = 'running';

-- 4. 准入策略与工单契约表：管家 Bot 治理外部触发与任务范围边界
CREATE TABLE admission_policies (
  repo_cwd        TEXT PRIMARY KEY,
  inferred_mode   TEXT NOT NULL DEFAULT 'observe' CHECK (inferred_mode IN ('off', 'observe', 'auto')),
  bot_actors_json TEXT NOT NULL DEFAULT '[]',    -- 外部机器人账号列表，严格禁止触发执行或终止工单
  updated_at      TEXT NOT NULL
);

CREATE TABLE tickets (
  id              TEXT PRIMARY KEY,              -- t + 8 hex
  session_id      TEXT NOT NULL REFERENCES sessions(id),
  repo_cwd        TEXT NOT NULL,
  title           TEXT NOT NULL,
  mandate_json    TEXT NOT NULL,                 -- { goal, allowed_paths, forbidden_actions, acceptance_criteria }
  source_kind     TEXT NOT NULL CHECK (source_kind IN ('explicit_user', 'inferred_event')),
  basis_msg_id    INTEGER REFERENCES messages(id),
  status          TEXT NOT NULL CHECK (status IN ('proposed', 'active', 'blocked_on_user', 'done', 'cancelled')),
  created_at      TEXT NOT NULL,
  updated_at      TEXT NOT NULL
);
-- 说明：1 个 Ticket 承载用户批准的 Mandate，统领该需求从分诊到验收的完整生命周期；
-- 其下可挂接多个不同厂商的 Run（如调研 -> 编码 -> 评审）。对于简单单次任务，dispatch_task 可自动创建并绑定单任务 Ticket。

-- 5. 任务运行表：由主控会话派发或用户创建的独立编码任务
CREATE TABLE runs (
  id               TEXT PRIMARY KEY,             -- r + 8 hex
  ticket_id        TEXT REFERENCES tickets(id),  -- 归属工单（绑定 Mandate 范围边界）
  session_id       TEXT NOT NULL REFERENCES sessions(id), -- 所属会话（中枢派发的挂在 hub 会话下）
  basis_msg_id     INTEGER REFERENCES messages(id),       -- 强绑定授权该任务的用户消息
  request_id       TEXT NOT NULL,
  vendor           TEXT NOT NULL CHECK (vendor IN ('codex', 'claude')),
  model            TEXT,
  instructions     TEXT NOT NULL,
  context_refs_json TEXT,                        -- 主控传入的前序 SHA、摘要与参考文件路径
  remote_url       TEXT NOT NULL,
  base_branch      TEXT NOT NULL,
  base_sha         TEXT NOT NULL,
  status           TEXT NOT NULL CHECK (status IN ('pending', 'running', 'done', 'interrupted', 'failed', 'needs_review')),
  current_attempt  INTEGER NOT NULL DEFAULT 0,
  detail           TEXT,
  result_sha       TEXT,
  observed_sha     TEXT,
  result_source    TEXT CHECK (result_source IN ('agent', 'user')),
  resolution       TEXT,
  created_at       TEXT NOT NULL,
  updated_at       TEXT NOT NULL
);
CREATE UNIQUE INDEX idx_runs_idempotent_req ON runs(session_id, request_id);
CREATE UNIQUE INDEX idx_runs_one_active_per_session
  ON runs(session_id) WHERE status IN ('pending', 'running') AND basis_msg_id IS NULL;
-- 注：主控会话(hub)允许同时派发并跟踪多个不同 repo/分支的并发子任务，受全局/单仓库并发配额限制；普通 chat 会话维持 R2 单活任务约束。

-- 6. 尝试世代表：每次开始或重试生成独立分支 agent/<runId>/a<attempt_no>
CREATE TABLE attempts (
  id               INTEGER PRIMARY KEY AUTOINCREMENT,
  run_id           TEXT NOT NULL REFERENCES runs(id),
  attempt_no       INTEGER NOT NULL,
  branch           TEXT NOT NULL,                -- agent/<runId>/a1, a2, ...
  worktree_path    TEXT NOT NULL,
  agent_thread_id  TEXT,                         -- 本次 attempt 内可做最多 2 次纠偏续接
  corrections      INTEGER NOT NULL DEFAULT 0,
  status           TEXT NOT NULL CHECK (status IN ('prepared', 'running', 'completed', 'cancelled', 'superseded', 'interrupted', 'failed')),
  created_at       TEXT NOT NULL,
  ended_at         TEXT,
  UNIQUE(run_id, attempt_no)
);

-- 7. 交付签收表：每个 run 至多只能有一条 accepted = 1 的记录
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
CREATE UNIQUE INDEX idx_results_one_accepted ON results(run_id) WHERE accepted = 1;

-- 8. 原始事件流与 SSE 增量日志表
CREATE TABLE events (
  id               INTEGER PRIMARY KEY AUTOINCREMENT,
  turn_id          INTEGER NOT NULL REFERENCES turns(id),
  event_kind       TEXT NOT NULL,
  payload_json     TEXT NOT NULL,
  created_at       TEXT NOT NULL
);

CREATE TABLE change_log (
  seq              INTEGER PRIMARY KEY AUTOINCREMENT,
  topic            TEXT NOT NULL,                -- 'message' | 'turn' | 'ticket' | 'run' | 'attempt' | 'hub_receipt' | 'inbox'
  entity_id        TEXT NOT NULL,
  payload_json     TEXT NOT NULL,
  created_at       TEXT NOT NULL
);
```

### 4.2 跨平台进程管控与文件系统适配（`polyclave-os`）

| 能力 | macOS（及类 Unix）实现 | Windows 实现 |
|---|---|---|
| **进程树硬隔离与防逃逸** | 创建独立进程组（`process_group(0)`），在 `polyclaved --supervisor-exec` 与父进程之间建立匿名控制管道：父进程强杀瞬间内核关闭管道写端，包装器利用 `kqueue`/`poll` 0ms 感知 EOF 并执行 `SIGTERM` -> 2s 宽限期 -> `SIGKILL` 清空整组。 | 父进程调用 `CreateJobObjectW` 并设置 `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`（**不开启** `BREAKAWAY_OK`），将受控子进程绑入 Job。无论代理工具怎么新建控制台或拉起 `git.exe`，句柄关闭或主动 `TerminateJobObject` 时由内核原子杀光整棵进程树。 |
| **进程身份防 PID 复用** | 通过 `sysctl` / `proc_pidinfo`（必要时辅以固定英文 UTC `ps`）获取进程精确启动时间，结合 `kern.boottime` 秒微秒数校验是否跨重启。 | 通过 `OpenProcess` + `GetProcessTimes` 读取精确到 100ns 的 `CreationTime`，结合 `GetTickCount64` 推导系统本次开机 Epoch。 |
| **启动四步握手** | 1. SQLite 写 `prepared`；2. 启动受控包装器回传 `ready` 与进程标识；3. SQLite 更新为 `authorized`；4. 父进程写入 `go\n` 放行。 | 完全相同的四步协议，通过匿名标准句柄/命名管道传递 `ready` 与 `go`，确保 Windows 下未放行崩溃同样 100% 确定从未启动。 |
| **路径与沙箱根规范** | 统一调用 `dunce::canonicalize`（底层等价于 `realpath`），消除 `/tmp` -> `/private/tmp` 符号链接导致的 Codex 沙箱拒绝。 | 统一调用 `dunce::canonicalize` **自动剥离标准库产生的 `\\?\C:\...` UNC 前缀**，避免 `git` 与 Node 类 CLI 报错；用 `which` 自动定位 `.exe` / `.cmd` / `.bat`。 |
| **隔离目录链接共享** | `skills` 目录与 `auth.json` 均使用标准 Unix symlink。 | **`skills` 目录共享使用免管理员权限的 NTFS Junction（`junction` crate）**；单文件凭据（如 `auth.json`）优先使用 NTFS 硬链接，跨卷时在每次启动前以受控只读副本同步。 |

### 4.3 Attempt 世代隔离、取消与崩溃恢复判定表

1. **取消不再锁死主控或工作会话**：
   - 当用户在界面点击 `Stop turn` 取消一个正在运行的主控推理或子任务轮次时：`polyclaved` 立即吊销该轮次的 `mcp_token`、终止进程树/Job Object，并在同一事务内将 `turns` 标为 `cancelled`、将当前 `attempts` 标为 `cancelled`。
   - **会话保持可用状态**，用户可直接发下一条消息修正需求或重新触发运行。
   - 当对某个 `run` 点击重试或主控决定再试一次时，宿主创建 **`attempt_no + 1`**，分配全新分支 **`agent/<runId>/a<n+1>`** 和独立工作树 `worktrees/<runId>-a<n+1>/`，并将上一轮残余的本地分支与工作区记为 `superseded`（只留作审计追踪或清理，绝不与新尝试混写）。
   - 对 Windows 而言，由于 Job Object 保证了 `writer_check` 可可靠归零（`excluded`），取消后远端没有分支即可直接安全重试；在 macOS 上若因 `POSIX_SPAWN_SETSID` 导致工具可能脱离进程组继续推送，因为新尝试使用全新的 `agent/<runId>/a2` 分支，旧写入者即使滞后推送到 `a1`，**绝对无法污染 `a2` 分支**；只有在用户决定将 `a1` 标记为待裁决时，才进入右侧 `Needs you`。
2. **全区间 `commit_summary` 与 R1 核证**：
   - `check_run_branch` 检查目标仓库 `agent/<runId>/a<n>` 分支：`base_sha` 必须为 `tip` 的祖先、`rev-list --count base_sha..tip > 0`，且区间内每一个提交的 `Run-Id` git trailer 均等于 `<runId>`。
   - **修复 v1 遗漏**：`commit_summary` 在远端执行 `git diff --numstat base_sha..sha` 与 `git log --format=%H%x09%s base_sha..sha`，在用户或主控查看“改了什么”时展示**整个任务分支相对于起点的完整累计改动文件和全部提交主题**，不再只看最后一次提交。
3. **统一判定表（`classify_run`）**：

| `writer_check` | 已签收 `accepted` 记录 | 远端 `agent/<runId>/a<n>` 分支检查 | `classify_run` 结论 | 下一步动作 |
|---|---|---|---|---|
| `possible`（同次开机强杀且无法排除脱逃写入） | 任意 | 任意 | `needs_review` | 标为待核查；提示旧 Attempt 可能存在延迟写入，允许用户切换新 Attempt 重试或手工核对 SHA 后采纳 |
| `excluded` | 无 | 缺失 (`absent`) | `retryable` (`interrupted`) | 安全：可自动或由主控决定开启 `attempt + 1` |
| `excluded` | 无 | 存在 (`present`) | `needs_review` | 推送了但未完成签收，列出全区间改动等待人工 `accept` / `fail` / `allow_retry` |
| `excluded` | 有 (`sha = S`) | 存在且 `tip == S` 且全区间合法 | **`done`** | 外部核证通过，写入 `hub_receipt` 回传主控会话并触发唤醒 |
| `excluded` | 有 (`sha = S`) | 缺失 / `tip != S` / 缺 Trailer | `needs_review` | 状态不一致，禁止自动标完成，交 `Needs you` 裁决 |
| 任意 | 任意 | 远端读取失败 (`error`) | `check_failed` (`interrupted`) | 保留原状，记录错误详情待重试核查 |

### 4.4 主控会话引擎与下层 Worker 适配器（`polyclave-hub` & `polyclave-adapters`）

#### 主控会话双模驱动接口（`HubModelProvider`）

```rust
#[async_trait]
pub trait HubModelProvider: Send + Sync {
    async fn run_turn(
        &self,
        ctx: HubTurnContext,          // 由 polyclave-store 组装的完整权威上下文与可用工具声明
        sink: mpsc::Sender<HubDelta>, // 流式推送思考过程、正文增量与工具意图
        cancel: CancellationToken,
    ) -> Result<HubTurnOutput, ProviderError>;
}
```

1. **模式一：`ApiHubProvider`（原生 API 直连，首选默认）**：
   - 支持配置任意兼容 OpenAI Chat Completions / Responses 或 Anthropic Messages 的端点，**内置 DeepSeek 官方预设（如 `https://api.deepseek.com`，模型 `deepseek-v4.1`）** 以及本地 Ollama 兼容接口。
   - **密钥安全**：API Key 仅通过系统钥匙串（`keyring` crate）或宿主的环境变量读取，只存在于 `polyclaved` 进程内存与系统凭据库，绝不写进 `polyclave.sqlite`、绝不打印进 SSE/日志、绝不注入下层任何 Worker CLI 环境变量。
   - **原生思考链与 Tool Calling**：流式解析 `delta.reasoning_content` 与 `delta.content`，写入 `messages.reasoning` 与 `messages.text`；模型发起的工具调用（如 `dispatch_task`）直接在宿主内存态执行并返回给同一轮 API 续接，零子进程冷启动延迟。
2. **模式二：`CliHubProvider`（借用 `codex` 或 `claude` CLI，会话主权绝不外流）**：
   - 当用户希望使用已订阅的 `codex` 或 `claude` 作为主控模型的推理引擎时启用。
   - **严格防脱管机制（“借脑不交权”）**：
     - 每次发起主控轮次时，宿主**不直接把主控权力交给外部 CLI 的自由会话目录**，而是使用独立空临时只读目录作为沙箱 cwd，只注入主控申请类的只读/派活 MCP。
     - **借用 `claude` CLI 精确参数**：`claude -p --verbose --output-format stream-json --include-partial-messages --strict-mcp-config --mcp-config <0600_json_path> --setting-sources "" --tools "" --allowedTools mcp__host__... --max-turns <n>`，通过环境变量 `CLAUDE_CONFIG_DIR` 指向私有隔离目录；`--tools ""` 彻底剥离写文件与 Bash 等内置工具，流式解析 `system/init` → `stream_event` → `result` 帧。
     - **上下文由宿主从 `polyclave.sqlite` 现拼注入**：包含系统角色提示、最新 `rolling_summary`、最近 N 轮完整对话（含先前 API 或其他厂商轮次的记录）、活跃 `Ticket` 与 `runs` 状态卡片以及用户最新消息。
     - **回复强行复制回本项目**：外部 CLI 流式输出完成后，宿主解析其最终回复文本、提取思考摘要与工具调用记录，**完整复制一条带 `provider_label = "codex_cli/..."` 或 `"claude_cli/..."` 的正式记录写入 `polyclave.sqlite` 的 `messages` 表**。外部 CLI 的本地临时文件随时可删，项目的开发进度、设计推导和对话历史 100% 永久留在 Polyclave 本地。
   - **长对话摘要压缩**：当 `messages` Token 估算接近模型窗口阈值时，宿主自动触发一次纯摘要调用更新 `sessions.rolling_summary`，确保切换模型或跨周续聊时上下文连贯。

#### 主动管家事件分诊循环（Proactive Bot Triage & Attention Budget）

主控会话以单线程事件分诊循环（Event Triage Loop）运行，统一消费三类入站事件：
1. **本人显式消息（`explicit`）**：来自已配对 Tauri 桌面端或 Tailscale 私有客户端的 `source = 'user'` 消息，直接具备授权资格（可生成 `active` 状态 `Ticket` 并绑定为 `basis_msg_id`）。
2. **子智能体终态与阻塞回执（`worker_receipt`）**：下层 `Run` 进入 `done` / `failed` / `needs_review` 或上报 `open_questions` 时写入主控收件箱。
3. **仓库与系统推断事件（`inferred`）**：受 `admission_policies` 严格约束——`off` 直接丢弃；`observe`（默认）仅由主控 Bot 提炼成 `proposed` 状态的工单提案卡片等待用户点击确认，**绝不自主开工**；`auto` 仅放行白名单授权用户的明确触发标识，任何外部机器人账号（`bot_actors_json`）的发言一律不得触发或终止工单。

**注意力预算（Attention Budget）汇报纪律**：下层 `Run` 在 `Mandate` 边界内正常推进或 2 次以内自动纠偏时，管家 Bot 仅静默更新 `Ticket` 进度条，不弹窗打扰；只有当整张 `Ticket` 验收完成、遇到 `direction` 级架构分叉、检测到超出 `allowed_paths` 的越界改动或进入 `needs_review` 时，才主动发出高信噪比决策卡片并触发私有通知。

#### 子项目 1 工具集划分

- **主控会话（`hub`）可用工具**：
  - `create_or_update_ticket({ basis_msg_id, repo_cwd, title, goal, allowed_paths, forbidden_actions?, acceptance_criteria? })`：将用户授权固化为带范围边界的 `Ticket`（含 `Mandate`）。
  - `dispatch_task({ basis_msg_id, ticket_id?, vendor, model?, repo_cwd, base_branch, title, instructions, context_refs? })`：核实 `basis_msg_id` 为来自客户端的真实 `source = 'user'` 消息（主控不可凭空捏造授权），校验仓库预检（支持本机 bare remote 与普通本地上游 git 仓库双模预检）后启动 Worker Run；未传 `ticket_id` 时自动创建并绑定单任务 Ticket。
  - `get_run_status({ run_id?, ticket_id? })`：查询当前各并行/历史子任务状态与全区间改动摘要。
  - `consult({ vendor, model?, question, context_paths? })`：单层只读跨厂商咨询（禁止被咨询方再次 `consult` 或 `dispatch_task`，单轮最多 5 次）。
- **下层编码 Worker（Codex / Claude）可用工具**：
  - 仅挂载宿主 HTTP MCP Server 提供的 **`submit_result`**、**`report_progress`** 与只读单层 **`consult`**。**物理不提供 `dispatch_task` 与 `create_or_update_ticket`**，从协议层杜绝 Worker 私自派活。
  - 宿主向外部 CLI 注入 HTTP MCP 的标准配置格式（权限 `0600`）：`{ "mcpServers": { "host": { "type": "http", "url": "http://127.0.0.1:<port>/mcp", "headers": { "Authorization": "Bearer <mcp_token>" } } } }`。
  - 适配方式（子项目 1 先接 2 家）：
    - **Codex**：先沿用实测极稳的每轮 `codex exec --json` / `resume` + `--ignore-user-config` + 私有 `CODEX_HOME`（仅链 `auth.json` 与 `skills`），预留 `codex app-server` 长驻连接配置开关。
    - **Claude Code**：使用 `claude -p --verbose --input-format stream-json --output-format stream-json` + `--mcp-config <temp-file> --strict-mcp-config --setting-sources ""` + `CLAUDE_CONFIG_DIR` 隔离视图（仅链登录态与 `~/.claude/skills`）。
    - **`--bare` 关键避坑规则（来自 `claude-code-probe` 调研验证）**：Claude Code CLI 的 `--bare` 参数会禁用订阅 OAuth 登录、强制要求 `ANTHROPIC_API_KEY`。因此当用户选择使用 Claude 订阅登录时，**严禁传递 `--bare`**（改用 `CLAUDE_CONFIG_DIR` + `--strict-mcp-config` + `--setting-sources ""` 完成配置隔离）；仅在显式配置 `ANTHROPIC_API_KEY` 计费模式下才启用 `--bare`。

#### 自动回传总主会话（Closed-Loop Dispatch）

当任一子智能体 Run 经过 `classify_run` 进入 **`done`**、**`failed`** 或 **`needs_review`** 终态时：
1. 宿主在 `hub` 会话写入一条 `source = 'host'` 的结构化回执消息（`metadata_json` 含 `run_id`、`attempt_no`、`sha`、`commit_summary` 统计、`agent_summary`、`open_questions`）。
2. 若主控会话当前空闲且配置为“自动唤醒汇报”，宿主自动推进一轮 `hub` 推理，让主控模型阅读该回执并决定是向用户汇报验收结论，还是直接带着前序 `sha` 与 `context_refs` 派发下一位智能体（例如从 Codex 编码完交回主控，主控再派 Claude 做代码审查）。

### 4.5 HTTP + SSE API 与 Tauri v2 + Leptos 界面契约

#### 守护进程接口（`polyclaved`）

- 默认仅绑定 `127.0.0.1:4310`（支持 `--port 0` 与 `--state-dir`）。
- 核心 REST 路由：
  - `GET /api/health`（返回 `instance_id`、版本、已启用厂商列表与可用性探测）
  - `GET /api/sessions` / `POST /api/sessions` / `PATCH /api/sessions/:id/provider`（随时切换主控会话的 `api` / `codex_cli` / `claude_cli` 及模型）
  - `GET /api/sessions/:id/messages` / `POST /api/sessions/:id/messages` / `POST /api/sessions/:id/cancel`
  - `GET /api/runs` / `GET /api/runs/:id`（含全区间 `commit_summary`）/ `POST /api/runs/:id/retry` / `POST /api/runs/:id/resolve`
  - `GET /api/events`（SSE 流：支持请求头 `Last-Event-ID`，先从 `change_log` 回放断点之后的所有持久化状态变更，再无缝衔接实时流式 token delta 与进度心跳）

#### Tauri v2 原生外壳 + Leptos WASM 界面分工

- **`src-tauri`（原生 Rust 薄壳）**：
  - 启动检测 `127.0.0.1:4310/api/health`，若未运行则在后台无窗拉起 `polyclaved` 守护进程（关掉窗口时守护进程继续常驻，可经系统托盘一键唤醒或完全退出）。
  - 管理本地操作系统钥匙串（`keyring`）：用户在设置页填写 DeepSeek 等 API Key 时，前端通过 Tauri Command 直接存入 macOS Keychain / Windows Credential Manager，或安全写入本地加密凭据通道通知 `polyclaved` 重载；钥匙串读取接口只向前端返回 `configured: bool` 与脱敏尾号，绝不回传明文 Key。
  - 注册 Tauri 原生桌面通知与系统托盘徽标（有 `needs_review` 或主控完成汇报时触发原生弹窗）。
- **`ui`（Leptos CSR WASM，100% Rust）**：
  - 严格继承 `DESIGN.md` 定义的色板（`#000000` 底、`#09090a` 侧栏、荧光绿 `#2dff5c` / 荧光橙 `#ff6a00` 仅用于运动状态与警告）、字体阶梯与间距。
  - **1440px 桌面三栏**：左栏「主控会话（Hub）+ 活跃/历史子任务树（带厂商/模型徽章）」；中栏「统一消息流（含用户消息、可折叠推理块、子智能体回执卡片、流式工具事件）+ 底部带模型/提供方快速切换器的输入框」；右栏「Needs you 待裁决区 + 当前 Run 全区间改动详情」。
  - **<860px 窄屏 / 375px 手机预览三标签**：顶部状态条 + 底部三标签（`对话` / `动态与任务` / `待你处理`），所有可交互控件触控高度 ≥ 44px，零横向溢出。

### 4.6 测试与跨平台验收标准

1. **确定性离线测试替身（全纯 Rust，无 Node 依赖）**：
   - `fake-agent` 测试二进制：行为由 JSON 脚本与全局有序轮次锁驱动，支持管道/文件屏障（`wait_barrier`）、模拟提交加 Trailer 推送、`submit_result`、以及故意启动脱离控制组的孤儿写入子进程（覆盖 `tests/proc.spec.ts` 与 `tests/recovery.spec.ts` 全套边界场景）。
   - `fake-llm-server` 测试夹具：基于本地回环 `axum` 服务模拟 OpenAI 兼容 SSE 流式响应、`reasoning_content` 与 `tool_calls`，离线验证 `ApiHubProvider` 的多轮调度闭环与失败取消。
   - **硬性纪律**：所有等待一律基于进程退出事件、Channel 或文件/TCP 屏障，**严禁固定 `sleep` 凑时序**；`legacy-node/tests/` 中的核心不变量用例须逐条在 `cargo test` 中建立对应测试后，方可淘汰旧目录。
2. **GitHub Actions 双平台 CI 矩阵**：
   - 在 `macos-latest` 与 `windows-latest` 上并行运行：
     - `cargo fmt --all -- --check`
     - `cargo clippy --workspace --all-targets -- -D warnings`
     - `cargo test --workspace`
     - Leptos WASM 前端编译校验（`cargo check -p polyclave-ui --target wasm32-unknown-unknown`）与 `scripts/privacy-scan.sh`。

---

## 5. 后续子项目路线图概览（子项目 2～4）

每个子项目各自拥有独立的设计补充与分步实施计划，按序推进，避免战线失控：

- **子项目 2：多模型内阁 `council` + 真实 GitHub PR 门禁与 `SCM Observer`**
  - 接入 **Antigravity (`agy`)** 与 **DeepSeek Harness (`dsh`)** 适配器。
  - 实现 `council` 原子宏流程：主控发起方案讨论 → 多厂商并行独立输出结构化 `Proposal` → 代码剥离厂商标识呈递“方案甲/乙/丙” → `direction` 级交用户拍板、`detail` 级结合偏好决策。
  - 实现 `pr_gate` 与后台 `SCM Observer`：三层 `touched_paths` 范围契约校验；强制异构模型 Reviewer 在一次性 Worktree 中评审确切 `head_sha`；Bot 账号与 Reviewer 账号物理隔离、显式传 `commit_id` 批准并先落台账再开 auto-merge；PR/CI 失败或评论变更实时作为事件推回主控会话中枢。
- **子项目 3：Tauri v2 移动双端（iOS / Android）与 Tailscale 私有通知配对**
  - 编译并打包 iOS 与 Android Tauri v2 客户端，适配真机 375px 触控交互与深色系统状态栏。
  - 实现扫码 10 分钟一次性票据换长期设备凭证、`tailscale serve` 身份头双重验证与宿主端设备吊销。
  - 坚持纯私有触达边界（拒绝第三方 IM 云）：移动端访问走 Tailscale 端到端加密通道（原生 App + 响应式 Web 备选）；远端提醒默认走**运行在用户本机或 Tailscale 私有网内的开源自托管通知渠道**（如自托管 `ntfy` / UnifiedPush 或自托管 Matrix 服务），推送载荷不附带敏感明文正文，App 收到唤醒后经 Tailscale `Last-Event-ID` 增量拉取完整内容，确保零对话内容外泄。
- **子项目 4：CUA 桌面互斥锁与本地运行台账进化循环**
  - 实现整机单把 CUA 互斥锁（检测键鼠空闲、用户桌面随时一秒收回）。
  - 基于本地 `polyclave.sqlite` 历史派活、返工次数、评审打回率与运行耗时，通过本地模型或 API 定期总结并生成版本化、可一键回滚的路由表与 Role Brief 优化建议（严格禁止触碰宿主门禁代码与用户全局 `skills`）。
