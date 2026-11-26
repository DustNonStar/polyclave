# Polyclave v5 设计规范：全栈 Rust 守护进程 + Tauri v2 轻量中枢控制台

- 日期：2026-11-26
- 状态：v5 正式设计草案（已吸纳首轮深度设计审查全部 P0/P1 修订，待用户最终签收）
- 取代：`docs/superpowers/specs/2026-11-25-agent-console-design-v4.md` 中的 Node.js 26 后端、Capacitor 手机壳、纯外部 CLI 托管总主会话模式，以及“点取消即冻结整个会话”的旧交互。
- 仓库归档策略：本规范面向全栈 Rust 演进。进入阶段 1A 实施时，先将现有 Node 26 实现移入 `legacy-node/` 归档对照（用于逐条迁移现有的 133 个幂等、竞态与崩溃恢复边界测试），保留 `docs/superpowers/research/` 与 `DESIGN.md` 资产；阶段 1A 验收通过后移除 `legacy-node/`（2026-11-26 用户决定）。

---

## 1. 文档定位与演进背景

Polyclave 的核心使命始终不变：**在本机运行的、绝对私有可控的多厂商 Agent 中枢控制台**——不把代码、会话和推送内容交给第三方云控制面，不照信子智能体自报的“我改完了”，由宿主根据外部 Git/远端状态硬核证结果，由总主会话中枢统一汇总上下文并调度各家最擅长的编码与评审 Agent。

### 1.1 从 v4 继承的核心原则（不变）

1. **R1 — 推送不等于完成**：只有当本轮唯一有效令牌签收了唯一的 `submit_result`、远端/宿主裸镜像目标分支 tip 与签收 SHA 一致、`base_sha` 是其祖先、区间提交非空且全区间每个提交均携带 `Run-Id: <runId>` git trailer 时，任务才算 `done`。
2. **四步启动握手**：先落库 `prepared` 轮次，启动受控包装器并将操作系统进程标识落库更新为 `authorized` 后，才向控制管道发送 `go` 放行；`go` 之前崩溃 100% 判定为 `never_started`。
3. **每轮独立一次性 MCP 令牌与幂等结算**：每个任务轮次签发仅在当前轮次有效的 MCP Bearer Token；相同 `(session_id, request_id)` 幂等受理，相同 `submit_result` 幂等返回，不一致则拒收。
4. **物理隔离用户全局 Agent 配置**：启动外部 CLI 时使用私有隔离目录，仅通过系统链接或受控同步共享登录凭据与 `skills`（沿用 `cc-switch` 习惯），屏蔽用户全局杂乱 MCP 与自定义钩子，严禁把模型 API Key 或 GitHub 写权限 Token 注入不该持有的进程。
5. **信任模型与视觉语言**：安全边界防“Agent 幻觉、越界修改、误操作、本机任意网页跨站请求和间接提示词注入”，不防同机恶意用户态提权；客户端沿用 `DESIGN.md` 定义的黑银工业风与荧光绿/橙动态警示视觉体系。

### 1.2 横向吸收自外部项目的工程设计

结合 `OrchestratorInc/agent-orchestrator`（AO）、`stablyai/orca`（Orca）、`pingdotgg/t3code`（T3 Code）与 `deepseek-ai/dsh-superagent-prototype`（DSH Superagent）的架构对比与前期实测（见 `docs/superpowers/research/`），Polyclave v5 明确采纳与改造如下设计：

| 来源项目 | 采纳进 Polyclave v5 的设计 | Polyclave v5 的差异化改造 |
|---|---|---|
| **Agent Orchestrator (`ao`)** | 1. **后台守护进程与薄前端彻底分离**：`polyclaved` 独立持有状态、数据库与子进程树，关闭窗口任务不断。<br>2. **SQLite `change_log` + SSE `Last-Event-ID`**：每次事务同步记录增量序号，桌面休眠唤醒或手机经 Tailscale 重连时无损补齐状态。<br>3. **后台 `SCM Observer`**：轮询 GitHub PR/CI 状态与评审评论。<br>4. **世代分支隔离（Attempt 分支）**：每次运行尝试使用独立分支 `agent/<runId>/a<n>`。 | 1. 拒绝 AO “CI 失败直接自动打回 Worker”的黑盒回路，改为把 PR/CI 事件推回**总主会话中枢**，由中枢带全局上下文与配额预算决定重派、换模型还是询问用户。<br>2. 用 **Attempt 世代分支 + Windows Job Object / macOS 匿名管道守卫**取代 v4 “取消即冻结会话”的苛刻机制。<br>3. 零云控制面（无 `contracts/cloud`）、零遥测。 |
| **Orca (`stablyai/orca`)** | 1. **长驻协议优先**：Worker 侧优先走原生流式协议（Claude `stream-json`、Codex `app-server` 备选与 `exec`、AGY/DSH `acp`）。<br>2. **扫码一次性票据换长期凭证**的设备配对机制与宿主端撤销。<br>3. **基于启动时间戳确认进程身份**，拒绝纯 PID 复用误判。 | 绝不采信 Worker 自报完成（必须过 R1 外部核证）；绝不镜像或修改用户的全局 `~/.codex` / `~/.claude` 配置；推送绝不经第三方网关明文转发。 |
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
     - **借用 `codex` 或 `claude` CLI 作为主控模型提供方**：允许利用本地已登录的 Codex 或 Claude Code CLI 执行主控推理，但**绝不让外部 Agent 接管主控会话的状态或记忆**；宿主每次从本地数据库按固定静态前缀顺序组装规范上下文，以只读无文件修改权限调用 CLI，并在每轮结束后**把 CLI 的回复全文、推理摘要与调度意图完整复制归档回 `polyclave.sqlite`**。用户可在任意轮次安全切换模型而不丢失历史。
5. **中枢集中调度（信息流开放、控制流收口）**（2026-11-26 用户决定）：平时子智能体完成后，宿主核证结果并将交付摘要自动回传给主控会话，由主控会话在 `Ticket` 预算（`max_runs`）内决定下一步派谁（前序产物路径、SHA 与摘要通过 `context_refs` 传给后续智能体）；除 `council` 盲评阶段外不强制隔绝信息，但子智能体绝不相互私自派活。
6. **主控升级为“主动值守的个人管家 Bot（Personal Superagent）” + 纯私有 Tailscale 触达**（2026-11-26 用户决定）：主控不仅是被动聊天框，而是具备统一事件收件箱（Event Triage Loop）、准入策略（`Admission Policy`）与 `Ticket / Mandate` 预算控制的主动大脑中枢；所有交互与通知严格限定在自研 Tauri 客户端与 Tailscale 私有网自托管开源通知渠道内，不依赖任何第三方聊天软件。
7. **三类可直聊会话角色（`hub` / `manager` / `worker`）与全写操作 Worktree 隔离**（2026-11-26 用户决定）：保留主控管家（`hub`）、各家常驻顾问（`manager`）和任务执行会话（`worker`）三类均可直接对话的会话。`hub` 与 `manager` 严格只读看仓，任何文件修改一律隔离在 `worker` 的独立 Attempt Worktree 中，彻底取消直接在主仓库 `repo_cwd` 原位改文件的无保护聊天模式，从架构根源消除主工作区并发写坏风险。
8. **稳健里程碑切分（阶段 1A → 阶段 1B）**（2026-11-26 用户决定）：先交付 **阶段 1A（Rust 守护进程核心底座 + 双平台进程/存储/本地裸镜像 R1/Worker 闭环）** 并跑通双平台测试与真实 CLI 探针门禁、淘汰 `legacy-node/`；再交付 **阶段 1B（主动管家主控引擎 + 常驻顾问会话 + Tauri v2 桌面端与轻量 TS 界面）**。

---

## 3. Cargo Workspace 模块架构与边界

整个工程组织为单个 Rust Cargo Workspace + 轻量前端子目录，严格约束依赖单向流动：

```text
polyclave/
├── Cargo.toml                        # Workspace 清单与统一依赖版本锁
├── crates/
│   ├── polyclave-proto/              # 纯数据契约与状态枚举 (serde, ts-rs)，零 OS/IO 依赖，自动生成 proto.ts
│   ├── polyclave-os/                 # 跨平台抽象：进程组/Win32 Job Object、启动握手包装器、路径规范化、Junction/同步
│   ├── polyclave-store/              # SQLite 存储层 (rusqlite bundled)、单写者通道、WAL 事务、唯一约束、change_log
│   ├── polyclave-adapters/           # 下层执行器适配器 (Codex / Claude / AGY / DSH) + 宿主 HTTP MCP Server
│   └── polyclave-hub/                # 中枢引擎：HubModelProvider (API & CLI 回写)、只读看仓工具、Ticket 调度、宏流程与 Observer
├── bins/
│   └── polyclaved/                   # 守护进程二进制 (axum HTTP/SSE、Token 鉴权、裸镜像管理、--supervisor-exec 入口)
├── apps/
│   └── polyclave-app/
│       ├── src-tauri/                # Tauri v2 Rust 原生外壳：守护进程拉起、系统托盘、系统钥匙串 (keyring)、原生通知
│       └── ui/                       # 轻量 TypeScript + DESIGN.md CSS 界面（由 Tauri 打包，也可由 polyclaved 直接托管）
│           └── src/generated/proto.ts # 由 polyclave-proto (ts-rs) 在 cargo test 时自动生成
└── legacy-node/                      # 阶段 1A 实施期临时归档的旧 Node 26 实现与测试对照（1A 验收后删除）
```

### 各单元核心职责与边界

- **`polyclave-proto`**：定义 `SessionDto`、`MessageDto`、`TicketDto`、`RunDto`、`AttemptDto`、`ProviderProfileDto`、`SseEnvelope`、`ApiError` 等全部前后端传输类型，启用 `ts-rs` 导出强类型定义至 `apps/polyclave-app/ui/src/generated/proto.ts`。
- **`polyclave-os`**：屏蔽 macOS 与 Windows 的系统差异，暴露纯正平台原语：`spawn_supervised`、`process_identity`、`boot_epoch`、`kill_process_tree`、`canonical_real_path`、`resolve_cli_binary`、`prepare_isolated_link`。上层业务代码严禁出现零散系统条件编译分支。
- **`polyclave-store`**：唯一持有 `rusqlite::Connection` 的 crate。采用“单写者专属线程/通道 + 读连接池（`PRAGMA journal_mode = WAL`, `PRAGMA busy_timeout = 5000`）”，消除异步 handler 持锁阻塞与写争用；所有状态跃迁在 `BEGIN IMMEDIATE` 事务内完成，并将 `SQLITE_CONSTRAINT_UNIQUE`（`2067`）映射为领域错误（`BusyError` / `ConflictError` / `BudgetExceededError`）。
- **`polyclave-adapters`**：封装各家 CLI 的参数构建、事件流解析（转为统一 `AgentEvent`）与宿主内置 HTTP MCP Server（仅暴露 `submit_result`、`report_progress`、`consult`）。
- **`polyclave-hub`**：系统的中枢大脑，管理 `hub` / `manager` 会话的 `HubModelProvider`（原生 API 流式推理 + CLI 只读调用与回写归档）、只读看仓工具、宿主托管本地裸镜像管理、`Ticket` 准入与预算分诊循环、`classify_run` 外部核证、`Recovery` 启动恢复，以及后续阶段的 `council` / `pr_gate` 宏流程与 `SCM Observer`。
- **`polyclaved`**：无头后台服务进程；`fn main()` 最顶部首先检查是否携带 `--supervisor-exec`，若是则不初始化 Tokio/SQLite，直接进入纯同步轻量包装器逻辑。
- **`polyclave-app`**：薄客户端。`src-tauri` 负责拉起 `polyclaved`、读写本地 `local-auth.token` 与系统钥匙串、系统托盘与原生通知；`ui` 为轻量静态 TS/HTML/CSS，支持 Tauri 容器内运行与浏览器免装访问。

---

## 4. 阶段 1A 与 1B 详细技术规范

### 阶段划分与验收边界

- **阶段 1A（Rust 守护进程核心底座 + 双平台等价交付）**：
  - 交付 `polyclave-proto`、`polyclave-os`、`polyclave-store`、`polyclave-adapters`（Codex + Claude Worker 适配）、宿主本地裸镜像管理、`polyclaved`（HTTP/SSE + 本机 Token 鉴权）。
  - 迁移并跑通 `legacy-node/tests/` 全部核心幂等、竞态、启动握手、Worktree/R1 核证与崩溃恢复不变量测试，以及 GitHub Actions `macos-latest` + `windows-latest` 双平台 CI；完成真实 CLI 探针门禁（`POLYCLAVE_REAL_CODEX=1`、`POLYCLAVE_REAL_CLAUDE=1`）后移除 `legacy-node/`。
- **阶段 1B（主动管家主控引擎 + 常驻顾问 + Tauri v2 轻量 TS 桌面端）**：
  - 交付 `polyclave-hub`（`ApiHubProvider` 支持 DeepSeek `deepseek-v4.1` 等 + `CliHubProvider` 借脑回写 + 只读看仓工具 + `Admission / Ticket` 预算分诊循环 + `manager` 常驻顾问会话）。
  - 交付 `apps/polyclave-app`（Tauri v2 原生外壳 + 沿用 `DESIGN.md` 与 `Web Animations API` 动效的轻量 TS 界面），并在无头浏览器（1440px 与 375px）完成全部 UI/交互与动画验证。

---

### 4.1 状态目录与 SQLite Schema（`polyclave-store`，闭合全部 P0 约束）

#### 状态目录布局

默认路径为 macOS `~/.polyclave`、Windows `%USERPROFILE%\.polyclave`（可通过 `--state-dir` 或 `POLYCLAVE_STATE_DIR` 覆盖）：

- `polyclave.sqlite`（及 `-wal` / `-shm`）：唯一持久化状态库。
- `host.lock`：JSON 文件 `{ "pid": 12345, "started_at": "...", "instance_id": "...", "port": 4310 }`，原子排他创建/替换。
- `local-auth.token`：守护进程启动生成的 256-bit 随机本机客户端鉴权密钥，Unix 权限 `0600`，Windows 设置仅当前用户 SID 可读写 ACL。
- `mirrors/`：宿主为每个登记开发仓库托管的只追加本地裸镜像 `<repo-hash>.git`。
- `isolated-homes/`：按 `codex/`、`claude/`、`hub-cli/` 存放隔离运行配置目录。
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
  rolling_summary TEXT NOT NULL DEFAULT '',      -- 本地滚动压缩摘要（仅供 hub/manager 组装静态前缀上下文）
  agent_thread_id TEXT,                          -- 仅用于底层 CLI 续接参考；hub/manager 的真相源永远在本库 messages
  created_at      TEXT NOT NULL
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

-- 7. 轮次表（闭合 [P0-1] 与 [P0-3]：正交并发锁 + writer_check 严禁默认值）
CREATE TABLE turns (
  id               INTEGER PRIMARY KEY AUTOINCREMENT,
  session_id       TEXT NOT NULL REFERENCES sessions(id),
  run_id           TEXT REFERENCES runs(id),
  attempt_id       INTEGER REFERENCES attempts(id),
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
  ended_at         TEXT
);
-- 两把正交部分唯一锁：会话对话流互不阻塞，同时单会话/单 Run 各自严格串行
CREATE UNIQUE INDEX idx_turns_running_session
  ON turns(session_id) WHERE status = 'running' AND run_id IS NULL;
CREATE UNIQUE INDEX idx_turns_running_run
  ON turns(run_id) WHERE status = 'running' AND run_id IS NOT NULL;

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

-- 9. 原始事件流与增量通知表
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
| **进程树硬隔离与防逃逸** | 创建独立进程组（`process_group(0)`）；`polyclaved --supervisor-exec` 在 `fn main()` 最顶部拦截并直入纯同步逻辑（不启 Tokio/SQLite）。父进程发完 `go\n` 后**必须保持控制管道写端常开直至轮次结束**；父进程强杀时内核关闭写端，包装器 0ms 感知 EOF 并执行 `SIGTERM` → 2s → `SIGKILL` 清空整组。 | 必须以 **`CREATE_SUSPENDED`** 挂起标志创建子进程 → 调用 `CreateJobObjectW`（设 `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`，**不开启** `BREAKAWAY_OK`）并 `AssignProcessToJobObject` 绑定 → 再 `ResumeThread` 放行，消除创建竞态逃逸窗。句柄关闭或主动 `TerminateJobObject` 时由内核原子清空整棵进程树。 |
| **进程身份防 PID 复用与开机标识** | 通过 `sysctl` / `proc_pidinfo`（辅以固定英文 UTC `ps`）读取精确进程启动时间；结合 `kern.boottime` 秒微秒数校验跨重启。 | 通过 `OpenProcess` + `GetProcessTimes` 读取 100ns 精度 `CreationTime`；**严禁使用 `SystemTime - GetTickCount64` 反推（防休眠/校时漂移）**，改用内核 `NtQuerySystemInformation(SystemTimeOfDayInformation)` 的稳定 `BootTime` 并作时区中性处理。 |
| **启动四步握手** | 1. SQLite 写 `prepared`；2. 启动包装器回传 `ready` 与系统标识；3. SQLite 更新 `authorized`；4. 写入 `go\n`（并持续保活管道）。 | 完全一致的四步握手，经匿名句柄传递 `ready` 与 `go\n`，未放行前崩溃 100% 判定 `never_started`。 |
| **路径与沙箱根规范** | `dunce::canonicalize` 消除 `/tmp` 等符号链接前缀。 | `dunce::canonicalize` **剥离标准库 `\\?\C:\...` UNC 前缀**，避免 `git` 与 CLI 报错；`which` 解析 `.exe` / `.cmd` / `.bat`。 |
| **隔离目录链接共享** | `skills` 与 `auth.json` 使用标准 Unix symlink。 | `skills` 目录共享使用免提权 **NTFS Junction（`junction` crate）**；单文件凭据避免被 CLI 原子 rename 打断硬链接，改用目录级 Junction（若隔离边界允许）或轮次启动前/结束后受控同步副本。 |

### 4.3 宿主托管本地裸镜像、Attempt 世代与崩溃恢复（闭合 `[P0-3]` / `[P0-5]`）

1. **宿主托管的只追加本地裸镜像（R1/R3 在真实项目上的统一落地）**：
   - 无论 `repo_cwd` 的上游 `origin` 是 GitHub HTTPS/SSH URL 还是本地路径，宿主都自动在 `<state-dir>/mirrors/<sha256(real_repo_cwd)[..16]>.git` 初始化并维护一个裸仓，强制设置 `receive.denyDeletes=true` 与 `receive.denyNonFastForwards=true`。
   - 派发任务前，宿主从 `repo_cwd` 解析 `base_branch` 的 `base_sha` 并同步推送到该本地裸镜像；Worker 在 `network_access=false` 沙箱下只能推入 `agent/<runId>/a<n>` 到此本地镜像（完全保留无网不可篡改外部证据）。
   - 当某个 `Run` 核证通过进入 `done` 后，宿主自动执行 `git -C <repo_cwd> fetch <mirror_remote> <result_sha>:refs/heads/polyclave/<runId>`，用户可直接在本地仓库查看或合并（阶段 2 再由宿主用独立 Bot Token 推送到 GitHub 开 PR）。
2. **三类会话的取消与直聊规则**：
   - **`hub` 与 `manager` 会话**：运行在纯只读看仓工具下，任何时候点击 `Stop turn` 取消均**不冻结、不重演**，立即允许继续对话。
   - **`worker` 会话（隔离在独立 Worktree 内直聊与插话）**：
     - 用户在 `worker` 空闲时发送消息，直接在该 Attempt 的 `worktree_path` 内续接执行；
     - 用户在 `worker` 运行中点击 `Stop turn`：在 Windows 上 Job Object 确保 `writer_check = 'excluded'`，可原地继续直聊或重试；在 macOS 上若轮次已 `authorized`（`writer_check = 'possible'`），则当前 Attempt 标记为 `cancelled/superseded`，新指令或重试自动切到 **`attempt_no + 1`**（分支 `agent/<runId>/a<n+1>`、独立工作树 `worktrees/<runId>-a<n+1>`），确保残留 `setsid` 子进程绝不污染新尝试，更绝不触碰主仓库 `repo_cwd`。
3. **修复 `check_run_branch` 与全区间 `commit_summary`**：
   - `check_run_branch` 在校验 `base_sha..tip` 期间，若发现某中间提交缺少 `Run-Id` trailer，返回 `BranchCheck::Present { sha: tip, problem: Some(...) }`——**始终返回真实分支 `tip`**（问题详情里标明违规提交哈希），彻底修复旧版返回中间提交 SHA 导致无法手动 `Accept` 的死循环。
   - `commit_summary` 在本地镜像上执行 `git diff --numstat base_sha..sha` 与 `git log --format=%H%x09%s base_sha..sha`，向用户与主控展示**整个任务分支相对于 `base_sha` 的完整累计改动文件与全部提交主题**。
4. **统一判定表（`classify_run`，仅针对当前有效 `current_attempt` 分支 `agent/<runId>/a<current>`）**：

| `writer_check` | 当前 Attempt `accepted` 记录 | 镜像 `agent/<runId>/a<current>` 分支检查 | `classify_run` 结论 | 下一步动作 |
|---|---|---|---|---|
| `possible`（macOS 同次开机强杀且无法排除脱逃写入） | 任意 | 任意 | `needs_review` | 标为待核查；保留旧 Attempt 分支，允许用户切至 `attempt + 1` 重试或手工核对后采纳 |
| `excluded` | 无 | 缺失 (`absent`) | `retryable` (`interrupted`) | 安全：可自动或由主控决定开启 `attempt + 1` |
| `excluded` | 无 | 存在 (`present`) | `needs_review` | 推送了但未签收，展示全区间 `commit_summary` 等待人工 `accept` / `fail` / `allow_retry` |
| `excluded` | 有 (`sha = S`) | 存在且 `tip == S` 且全区间合法 | **`done`** | 核证通过，回传结果至主控并更新 `polyclave/<runId>` 本地引用 |
| `excluded` | 有 (`sha = S`) | 缺失 / `tip != S` / 缺 Trailer | `needs_review` | 状态不一致，禁止自动标完成，交 `Needs you` 裁决 |
| 任意 | 任意 | 镜像读取失败 (`error`) | `check_failed` (`interrupted`) | 记录错误详情待重试核查 |

（注：已被取代的旧世代 `a<k>` 若事后收到滞后推送，宿主仅写入事件日志留痕，不改变现行 Attempt 状态。）

### 4.4 主控与顾问引擎、只读看仓工具及 Worker 适配器（`polyclave-hub` & `polyclave-adapters`）

#### `HubModelProvider` 双模接口与防 Cache 失效上下文拼装

- **原生 API 模式（`ApiHubProvider`，首选默认）**：支持绑定在 `provider_profiles` 上的兼容端点（内置 DeepSeek `https://api.deepseek.com` / `deepseek-v4.1` 等），从系统钥匙串读 Key；流式解析 `reasoning_content` 与原生 Tool Call。
- **借用 CLI 模式（`CliHubProvider`，借脑不交权）**：
  - 每次从 `polyclave.sqlite` 现拼上下文调用外部 `codex` 或 `claude` CLI，并在轮次结束时**完整复制回复文本、思考摘要与工具调用意图写回 `messages` 表**。
  - **上下文静态前缀稳定规则**：组装提示词时严格按 **`[系统角色指令] → [rolling_summary] → [已固化历史轮次]`** 作为不可变前缀置前，把动态变化的 `Ticket / Run` 状态快照与本轮最新输入追加在最末尾，最大限度保住模型厂商的 Prompt Cache 命中率。
  - **Claude CLI 精确参数与 `--bare` 避坑规则（来自 `claude-code-probe` 调研）**：
    - 严禁使用 `--permission-mode plan`（实测不可靠且会触发内置规划流程）；统一使用 `--tools ""` 剥离全部内置写/Shell 工具，并用 `--allowedTools` 仅放行宿主提供的只读/调度 MCP 工具：
      `claude -p --verbose --output-format stream-json --include-partial-messages --strict-mcp-config --mcp-config <0600_json_path> --setting-sources "" --tools "" --allowedTools mcp__host__... --max-turns <n>`，配合 `CLAUDE_CONFIG_DIR` 隔离视图。
    - **禁止对订阅登录传 `--bare`**：Claude Code 的 `--bare` 会禁用 OAuth 订阅登录并强制要求 `ANTHROPIC_API_KEY`；仅在用户显式配置 API Key 模式时开启 `--bare`。
    - 宿主 HTTP MCP 配置模板（`0600`）：`{ "mcpServers": { "host": { "type": "http", "url": "http://127.0.0.1:<port>/mcp", "headers": { "Authorization": "Bearer <mcp_token>" } } } }`。

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

- 仅挂载宿主 HTTP MCP 的 `submit_result`、`report_progress`、`consult`（单层只读，深度=1）；**物理不提供 `dispatch_task` 与 `create_or_update_ticket`**。

### 4.5 Loopback 安全鉴权、SSE 流控与 Tauri v2 + 轻量 TS 界面

#### 本机安全与 HTTP/SSE 协议

- **强本机鉴权与 CSRF 防护（闭合 `[P0-4]`）**：
  - `polyclaved` 启动写入 `0600` 的 `local-auth.token`；**所有 `/api/*` 路由（含 `/cancel`、`/retry` 与 `/api/events`）必须携带 `Authorization: Bearer <token>`**（严禁经 URL query 传 token），并校验 `Host` 与严格 `Origin` 白名单（`tauri://localhost`、`http://tauri.localhost`、`https://tauri.localhost`、`http://127.0.0.1:<port>`）。
  - REST 路由仅允许切换已登记的 `(profile_id, provider_model)`，彻底封堵改写 `api_base_url` 窃取 Key 的攻击面。
- **双通道流式事件与 `change_log` 裁剪**：
  - 持久化状态变更（`message`、`turn`、`ticket`、`run`、`attempt`、`hub_receipt`、`inbox`）写入 `change_log`（保留最近 10,000 条，过期 `Last-Event-ID` 下发 `event: resync_required` 触发全量对齐）。
  - 高频流式 token delta 与思考增量仅走内存 `tokio::sync::broadcast`，不写入 SQLite 避免写放大。
  - 前端使用 **`fetch` + `ReadableStream` SSE 解析器**（而非不支持自定义鉴权头的原生 `EventSource`），按增量事件局部更新 DOM 并复用/演进防打断焦点保持机制（`withFocusPreserved`）。

#### Tauri v2 + 轻量 TS 界面分工

- **`src-tauri`**：自动拉起与保活 `polyclaved`；读取 `local-auth.token` 注入授权上下文；通过 `keyring` 管理 API Key（向前端仅回传 `configured: bool` 与掩码尾号）；提供系统托盘驻留与桌面原生通知。
- **`ui`**：继承 `DESIGN.md` 色板、圆角、字号阶梯与工业银灰视觉，使用 `Web Animations API` 实现卡片入场与运动态荧光提示；桌面三栏（左：主控管家 `hub` + 各家顾问 `manager` + 工单任务树；中：可折叠思考链 + 调度回执卡片 + 三角色直聊流；右：`Needs you` 待裁决区与全区间改动详情），`<860px` 切换三标签响应式布局。所有契约类型直接导入 `ts-rs` 自动生成的 `proto.ts`。

### 4.6 测试、双平台 CI 与实机门禁

1. **纯 Rust 确定性离线测试夹具**：
   - `fake-agent`：支持脚本化轮次锁、文件/管道屏障、模拟带 `Run-Id` trailer 提交推送、`submit_result` 及故意脱逃的子进程写入测试（覆盖 macOS 与 Windows 双平台恢复场景）。
   - `fake-llm-server`：本地回环 `axum` 模拟流式 SSE、`reasoning_content` 与 `tool_calls`，离线验证 `ApiHubProvider` 与 `Ticket` 配额拦截。
   - **硬性纪律**：严禁固定 `sleep` 凑时序；必须覆盖阶段 1A 迁移自 `legacy-node/tests/` 的核心不变量用例。
2. **真实 CLI 门禁（必须实跑关掉，不再悬空）**：
   - 在具备本机环境的机器上通过环境变量显式开启 `POLYCLAVE_REAL_CODEX=1` 与 `POLYCLAVE_REAL_CLAUDE=1` 完成实机验收；并在阶段 1B 启动前用实机探针验证一次 `CliHubProvider`（`--tools ""` 只读借脑）的实际延迟与工具调用准确率。
3. **GitHub Actions 矩阵**：
   - 在 `macos-latest` 与 `windows-latest` 上并行执行：`cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo test --workspace`（含 `ts-rs` 契约一致性检查）、UI TypeScript `tsc --noEmit` 与 `scripts/privacy-scan.sh`。

---

## 5. 后续子项目路线图概览（阶段 2～4）

- **阶段 2：多模型内阁 `council` + 真实 GitHub PR 门禁与 `SCM Observer`**
  - 接入 **Antigravity (`agy`)** 与 **DeepSeek Harness (`dsh`)** 适配器。
  - 实现 `council` 原子宏流程：主控发起方案讨论 → 各家 `manager` 并行独立输出结构化 `Proposal` → 代码剥离厂商标识呈递“方案甲/乙/丙” → `direction` 级交用户拍板、`detail` 级结合偏好决策。
  - 实现 `pr_gate` 与后台 `SCM Observer`：三层 `touched_paths` 范围契约校验；强制异构模型 Reviewer 在一次性 Worktree 中评审确切 `head_sha`；Bot 账号与 Reviewer 账号物理隔离、将本地镜像通过 R1 的分支推送到 GitHub、显式传 `commit_id` 批准并先落台账再开 auto-merge；PR/CI 失败或评论变更实时作为事件推回主控收件箱。
- **阶段 3：Tauri v2 移动双端（iOS / Android）与 Tailscale 纯私有通知配对**
  - 编译打包 iOS 与 Android Tauri v2 客户端，适配真机 375px 触控交互。
  - 实现扫码 10 分钟一次性票据换长期设备凭证、`tailscale serve` 身份头双重验证与宿主端设备吊销。
  - 坚持纯私有触达边界（拒绝第三方 IM 云）：移动端访问走 Tailscale 端到端加密通道（原生 App + 宿主静态托管响应式 Web 备选）；远端提醒默认走**运行在用户本机或 Tailscale 私有网内的开源自托管通知渠道**（如自建 `ntfy` / UnifiedPush 或自建 Matrix 服务），推送载荷不含敏感明文正文，App 唤醒后经 Tailscale `Last-Event-ID` 增量拉取完整内容。
- **阶段 4：CUA 桌面互斥锁与本地运行台账进化循环**
  - 实现整机单把 CUA 互斥锁（检测键鼠空闲、用户桌面随时一秒收回）。
  - 基于本地 `polyclave.sqlite` 历史工单、返工次数、评审打回率与运行耗时，通过本地模型或 API 定期总结并生成版本化、可一键回滚的路由表与 Role Brief 优化建议（严格禁止触碰宿主门禁代码与用户全局 `skills`）。
