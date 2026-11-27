# Polyclave v5 阶段 1A（Rust 守护进程核心底座 + 双平台等价交付）实施计划

> **给执行代理：** 按任务顺序逐项推进，每完成一项核对并通过全部校验后再提交。严格遵循 `docs/superpowers/specs/2026-11-26-polyclave-v5-rust-design.md` 的架构边界与不变量。

**目标：** 用纯 Rust Cargo Workspace（`polyclave-proto`、`polyclave-os`、`polyclave-store`、`polyclave-adapters`、`polyclave-core`、`polyclaved`）取代 Node.js 26 后端，实现 macOS 与 Windows 原生双平台进程树硬隔离、SQLite WAL 单写者通道与 FTS5 记忆库、宿主托管只追加本地裸镜像 + R1 外部核证、Attempt 世代隔离与崩溃恢复，完整迁移并跑通现有 133 个幂等/竞态/崩溃恢复不变量测试，完成真实 CLI 探针门禁后移除 `legacy-node/`。

**规格依据：** `docs/superpowers/specs/2026-11-26-polyclave-v5-rust-design.md`（§1–§4.3、§4.5–§4.6）。

---

## 全局硬性约束

- **依赖单向流动：** `polyclave-proto`（零 OS/IO） → `polyclave-os` / `polyclave-store` → `polyclave-adapters` → `polyclave-core` → `polyclaved`。严禁反向依赖或循环依赖。
- **平台原语集中于 `polyclave-os`：** 上层业务 crate 严禁出现零散 `#[cfg(target_os = ...)]` 条件编译分支。
- **真实路径规范：** 所有写入数据库或传给子进程的仓库/工作树/镜像路径必须经 `dunce::canonicalize` 规范化（Windows 剥离 `\\?\` UNC 前缀，macOS 消除 `/tmp` 等符号链接）。
- **四步启动握手：** `prepared` 落库 → 包装器回传 `ready` + OS 进程标识 → `authorized` 落库 → 发送 `go\n`（macOS 保活控制管道写端，Windows `CREATE_SUSPENDED` + Job Object `KILL_ON_JOB_CLOSE` 后 `ResumeThread`）。`go` 之前崩溃 100% 判定 `never_started`。
- **令牌与记忆隔离：** `TurnDto` 编译期物理剔除 `mcp_token`；`<state-dir>/tmp/mcp-turn-<id>.json`（`0600`/仅当前用户 ACL）在轮次结束、取消与恢复时**立即删除**；`<state-dir>/isolated-homes/<vendor>/` 下各家原生记忆与对话历史**永久保留**。
- **R1 核证与 `allow_retry` 升级：** 仅当本轮唯一有效令牌签收唯一 `submit_result`、镜像 `agent/<runId>/a<current>` tip 等于签收 SHA、`base_sha` 为其祖先、区间非空且全区间提交带 `Run-Id: <runId>` trailer 时才标 `done`。v5 允许旧世代分支仍存在于镜像时 `allow_retry`，将旧 Attempt 标为 `superseded` 并在下次 `retry` 时从 `base_sha` 干净开启 `a<n+1>`。
- **测试纪律：** 严禁固定 `sleep` 凑时序，必须使用进程退出事件、文件/管道屏障或条件信号；每次提交后必须通过 `cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`。

---

## 任务清单

### Task 1：归档旧实现、初始化 Cargo Workspace 与隐私扫描更新
- **文件：**
  - 移动 `src/`、`tests/`、`public/`、`package.json`、`pnpm-lock.yaml`、`tsconfig.json`、`vitest.config.ts` → `legacy-node/`（保留 `docs/`、`DESIGN.md`、`AGENTS.md`、`REVIEWER.md`、`HANDOFF.md`、`PLAN-ISSUES.md`、`handcheck/`）
  - 创建 `Cargo.toml`（Workspace 清单：`crates/polyclave-proto`、`crates/polyclave-os`、`crates/polyclave-store`、`crates/polyclave-adapters`、`crates/polyclave-core`、`bins/polyclaved`）
  - 更新 `.gitignore`（忽略 `target/`、`/legacy-node/node_modules/` 等）
  - 更新 `scripts/privacy-scan.sh`（豁免 `Cargo.lock` 的 400 KB 体积限制，保留本地绝对路径与密钥正则扫描）
- **提交信息：** `chore: archive legacy-node and initialize rust workspace`
- **验收：** `scripts/privacy-scan.sh` 输出 `clean`，`cargo metadata --no-deps` 成功解析全部 Workspace 成员。

### Task 2：`polyclave-proto` 纯数据契约与 `ts-rs` 导出一致性
- **文件：** `crates/polyclave-proto/Cargo.toml`、`crates/polyclave-proto/src/lib.rs`、`crates/polyclave-proto/src/dto.rs`、`crates/polyclave-proto/src/events.rs`、`crates/polyclave-proto/src/error.rs`
- **核心契约：**
  - 定义 `SessionDto`、`SessionRole`（`hub | manager | worker`）、`MessageDto`、`TicketDto`、`RunDto`、`AttemptDto`、`TurnDto`（**物理不含 `mcp_token`**）、`MemoryItemDto`、`ProviderProfileDto`、`SseEnvelope`、`ApiError`。
  - 启用 `ts-rs`，编写测试将类型导出至 `apps/polyclave-app/ui/src/generated/proto.ts` 并校验字段完整性（含编译期/序列化断言 `TurnDto` 不含 `mcp_token`）。
- **提交信息：** `feat(proto): add pure data contracts and ts-rs exports`
- **验收：** `cargo test -p polyclave-proto` 生成最新 `proto.ts` 且无未提交漂移。

### Task 3：`polyclave-os` 跨平台进程树管控、防 PID 复用与沙箱链接
- **文件：** `crates/polyclave-os/Cargo.toml`、`crates/polyclave-os/src/lib.rs`、`crates/polyclave-os/src/unix.rs`、`crates/polyclave-os/src/windows.rs`、`crates/polyclave-os/src/path.rs`
- **核心原语：**
  - 统一暴露：`spawn_supervised`、`supervisor_exec_entry`、`process_identity`、`boot_epoch`、`kill_process_tree`、`canonical_real_path`、`resolve_cli_binary`、`prepare_isolated_link`、`write_private_file`（`0600` / Windows 当前用户 SID ACL）。
  - **macOS/Unix：** `process_group(0)`；父进程向包装器发送 `go\n` 后保持控制管道写端常开直至轮次结束，EOF 时 0ms 触发 `SIGTERM` → 2s → `SIGKILL` 清空整组；`sysctl` / `proc_pidinfo` 读取启动时间与 `kern.boottime`。
  - **Windows：** `CREATE_SUSPENDED` 创建子进程 → `CreateJobObjectW`（`JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`，**禁开** `BREAKAWAY_OK`）→ `AssignProcessToJobObject` → `ResumeThread`；`GetProcessTimes` 读取 100ns `CreationTime`；`NtQuerySystemInformation(SystemBootEnvironmentInformation)` 读取 128-bit `BootIdentifier` GUID（回退 `BootTime - BootTimeBias`）；`junction` crate 创建免提权 NTFS Junction。
- **提交信息：** `feat(os): implement macOS/Windows process isolation, boot epoch, and path primitives`
- **验收：** 双平台单元测试覆盖：父进程强杀时子进程树自动退出、未发 `go\n` 关闭管道时目标进程 0 启动、`dunce::canonicalize` 剥离 UNC/符号链接。

### Task 4：`polyclave-store` 单写者 SQLite 通道、Schema、记忆库与 `change_log`
- **文件：** `crates/polyclave-store/Cargo.toml`、`crates/polyclave-store/src/lib.rs`、`crates/polyclave-store/src/schema.sql`、`crates/polyclave-store/src/writer.rs`、`crates/polyclave-store/src/memory.rs`
- **核心实现：**
  - 启用 `rusqlite` `bundled` + `fts5`；单写者专属线程通道（`BEGIN IMMEDIATE` 事务）+ WAL 读连接池（`busy_timeout = 5000`）。
  - 建表：规范 §4.1 全部 10 张表（`provider_profiles`、`sessions`、`hub_provider_threads`、`messages`、`admission_policies`、`tickets`、`runs`、`attempts`、`turns`、`results`、`memory_items` + `memory_items_fts` 虚表与同步触发器、`events`、`change_log`）。
  - 约束映射：将 `SQLITE_CONSTRAINT_UNIQUE`（`2067`）精确映射为 `BusyError` / `ConflictError` / `BudgetExceededError`；强校验 `idx_turns_running_session` 与 `idx_turns_running_run` 排除 `mode = 'consult'`。
  - `change_log` 保留最近 10,000 条，`since_seq` 越界判定返回 `ResyncRequired`。
- **提交信息：** `feat(store): add single-writer SQLite WAL store, FTS5 memory, and change_log`
- **验收：** `cargo test -p polyclave-store` 覆盖唯一锁并发冲突、轮内 `consult` 不自锁死、FTS5 检索与 `candidate` 写入隔离、`change_log` 裁剪触发 `ResyncRequired`。

### Task 5：`polyclave-adapters` CLI 适配器与宿主 HTTP MCP Server
- **文件：** `crates/polyclave-adapters/Cargo.toml`、`crates/polyclave-adapters/src/lib.rs`、`crates/polyclave-adapters/src/codex.rs`、`crates/polyclave-adapters/src/claude.rs`、`crates/polyclave-adapters/src/mcp.rs`
- **核心实现：**
  - **Codex Worker：** `codex exec --json` / `codex exec resume` + `--ignore-user-config`，`CODEX_HOME = <state-dir>/isolated-homes/codex`（持久保留对话与记忆），`sandbox_mode = "workspace-write"`、`network_access = false`、`writable_roots = [worktree_path, git_common_dir, mirror_remote]`，经 `POLYCLAVE_MCP_TOKEN` 传令牌。
  - **Claude Worker：** `claude -p --verbose --output-format stream-json --include-partial-messages --session-id <uuid>`（同 Attempt 续接 `--resume <uuid>`），`--strict-mcp-config --mcp-config <state-dir>/tmp/mcp-turn-<id>.json --setting-sources ""`（保留文件/Shell 工具，**不传 `--tools ""`**；订阅登录模式下**不传 `--bare`**），`CLAUDE_CONFIG_DIR = <state-dir>/isolated-homes/claude`。
  - **宿主 HTTP MCP Server：** 校验 Bearer `mcp_token`（仅当对应 turn 为 `running` + `authorized` + 当前 `host_instance_id` 时有效）；暴露阶段 1A 工具：`submit_result`（幂等签收）、`report_progress`、`consult`（单层只读，深度=1）、`memory_brief`、`memory_search`、`memory_propose`（仅写 `candidate`，物理不暴露 `dispatch_task` / `memory_commit`）。
- **提交信息：** `feat(adapters): implement Codex/Claude worker adapters and host HTTP MCP server`
- **验收：** 单元测试验证参数构造（含 `--` 与禁用 `--bare` 规则）、流式 JSON 解析为统一 `AgentEvent`、无效/过期 `mcp_token` 拒绝 401/403、`memory_propose` 越权限制。

### Task 6：`polyclave-core`（一）本地裸镜像、Worktree、分支检查与 `classify_run`
- **文件：** `crates/polyclave-core/Cargo.toml`、`crates/polyclave-core/src/lib.rs`、`crates/polyclave-core/src/mirror.rs`、`crates/polyclave-core/src/worktree.rs`、`crates/polyclave-core/src/classify.rs`
- **核心实现：**
  - `MirrorManager`：按规范化 `real_repo_cwd` 维护进程内异步互斥锁；初始化 `<state-dir>/mirrors/<sha256[..16]>.git`（强制 `receive.denyDeletes=true`、`receive.denyNonFastForwards=true`）；派发前不可变推送 `git -C <repo_cwd> push <mirror> <base_sha>:refs/bases/<runId>`；`done` 后回拉 `refs/heads/polyclave/<runId>` 且回拉失败绝不回滚 `done`。
  - `WorktreeManager`：仅从 `run.base_sha`（或 `refs/bases/<runId>`）干净创建 `<state-dir>/worktrees/<runId>-a<n>`。
  - `check_run_branch`：校验 `base_sha..tip` 全区间携带 `Run-Id: <runId>` trailer，缺 trailer 时**始终返回真实分支 `tip`** 与 `problem` 详情。
  - `commit_summary`：在本地镜像执行 `git diff --numstat base_sha..sha` 与 `git log` 返回累计改动。
  - `classify_run`：严格实现规范 §4.3 判定表。
- **提交信息：** `feat(core): add MirrorManager, WorktreeManager, check_run_branch, and classify_run`
- **验收：** 覆盖并发同仓派发互斥锁、缺 trailer 返回真实 tip、`done` 回拉失败不回滚等单测。

### Task 7：`polyclave-core`（二）`TurnRunner`、`RunManager` 与 `Recovery`
- **文件：** `crates/polyclave-core/src/runner.rs`、`crates/polyclave-core/src/runs.rs`、`crates/polyclave-core/src/recovery.rs`
- **核心实现：**
  - `TurnRunner`：四步握手、`--supervisor-exec` 控制管道保活、取消处理、轮次结束/取消后立即删除 `<state-dir>/tmp/mcp-turn-<id>.json`（绝不删除 `isolated-homes/`）。
  - `RunManager`：缺省 `ticket_id` 时同一事务自动创建 `max_runs=1` 的 `explicit_user` 工单与专属 `worker` 会话；`worker_chat` 绑定 `(run_id, attempt_id)`，终态（`done`/`failed`）严格拒绝 `409 Conflict`；`allow_retry` 将旧 Attempt 标为 `superseded` 并从 `base_sha` 开启 `a<n+1>`；`done` 或人工 `resolve` 自动写入 `verified` `progress`/`receipt` 记忆。
  - `Recovery`：启动时先清理遗留 `tmp/mcp-turn-*.json`，再按 `boot_epoch` 与 `process_identity` 判定 `writer_check` 并调用 `classify_run` 收敛旧运行态。
- **提交信息：** `feat(core): implement TurnRunner, RunManager, and startup Recovery`
- **验收：** 崩溃恢复、断电重开、`worker_chat` 409 拦截、`allow_retry` 世代切换单测全部通过。

### Task 8：`polyclaved` 守护进程、鉴权与 HTTP/SSE 路由
- **文件：** `bins/polyclaved/Cargo.toml`、`bins/polyclaved/src/main.rs`、`bins/polyclaved/src/auth.rs`、`bins/polyclaved/src/routes.rs`
- **核心实现：**
  - `fn main()` 最顶部优先检查 `--supervisor-exec`，若是则零 Tokio/SQLite 初始化直接进入 `polyclave_os::supervisor_exec_entry`。
  - 写入 `host.lock` 与 `0600`/ACL `local-auth.token`，stdout 打印 `http://127.0.0.1:<port>/#token=<token>`。
  - `axum` 路由：所有 `/api/*` 强制 `Authorization: Bearer <token>` + `Host` 与严格 `Origin` 白名单（`tauri://localhost`、`http://tauri.localhost`、`https://tauri.localhost`、`http://127.0.0.1:<port>`）；SSE 支持 `Last-Event-ID` 补发与 `resync_required` 事件，流式 delta 走内存 `tokio::sync::broadcast`。
- **提交信息：** `feat(daemon): implement polyclaved HTTP/SSE server and loopback auth`
- **验收：** HTTP/SSE 鉴权、Origin 拒绝、`Last-Event-ID` 断线重连补齐、`resync_required` 集成测试通过。

### Task 9：迁移 133 个不变量测试与配置双平台 GitHub Actions CI
- **文件：** `crates/*/tests/`、`tests/fixtures/fake_agent.rs`、`.github/workflows/check.yml`、`AGENTS.md`、`REVIEWER.md`
- **核心实现：**
  - 实现纯 Rust `fake-agent` 测试夹具（支持脚本化锁、屏障、带 `Run-Id` trailer 推送、`submit_result`、故意脱离进程组/作业对象的写入测试）。
  - 逐条对照 `legacy-node/tests/` 迁移全部幂等、竞态、启动握手、Worktree/R1 核证与崩溃恢复测试，显式记录 v5 `allow_retry` 允许旧世代分支存在于镜像时的语义升级。
  - 新增测试：`memory_items` FTS5 与隔离写入、轮内 `consult` 无自锁死、`worker_chat` 终态 409、`refs/bases/<runId>` 不可变推送。
  - 配置 `.github/workflows/check.yml`（`macos-latest` + `windows-latest` 矩阵）：`cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo test --workspace`、`proto.ts` 导出一致性检验、`scripts/privacy-scan.sh`。
- **提交信息：** `test: migrate phase1 invariants to Rust and enable macOS/Windows CI`
- **验收：** 双平台 CI 全绿，所有迁移用例无 `sleep`、无 flaky。

### Task 10：真实 CLI 探针门禁与移除 `legacy-node/`
- **验证命令：**
  - `POLYCLAVE_REAL_CODEX=1 cargo test --test real_codex`
  - `POLYCLAVE_REAL_CLAUDE=1 cargo test --test real_claude`
  - 核实真实 CLI 下 `isolated-homes/<vendor>/` 原生记忆持久留存、`tmp/mcp-turn-<id>.json` 自动清理、R1 核证通过。
- **清理：** 删除 `legacy-node/` 目录，运行 `scripts/privacy-scan.sh` 确认 `clean`。
- **提交信息：** `chore: pass real CLI gates and remove legacy-node`
