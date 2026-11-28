# Polyclave v5 真机验收前 P0 修复与验收计划

- **基线提交**：`main@4ad1521`（基于 `main@0b593c7` 四模型独立盲审实证 + `dsh-superagent-prototype` 交叉借鉴）
- **目标**：通过 1 轮聚焦的 3 批次修复，清零已确认的 10 项 P0 阻断与 2 项安全阀漏洞，顺带补齐高性价比真机排障 P1；全部跑绿本地检查与 macOS / Windows 双平台 CI 后，立即进入**单故事真机端到端验收**。
- **范围纪律**：Mesh 调度、桌面控制（CUA）、自我进化与社区蒸馏新功能继续暂停，本轮仅修主链路 P0 与 2 项安全阀（SV1/SV2）。

---

## 0. 核心架构不变量与本轮吸收的工程拼图

### 4 条架构不变量（先立不变量，再写补丁）
- **I1（单活 Gate 与已批准 SHA 不可变）**：同一个 Run 同一时刻至多存在一个活跃 PR Gate；一旦进入 `awaiting_human_approval` 或 `auto_merge_armed`，在显式撤回或完成前，严禁新建兄弟 Gate 或向同一 PR 推送新 head。
- **I2（返工提交继承 `from_sha`）**：返工 Attempt 的 `HostTools::commit_and_stage` 必须从持久化 `attempts.from_sha` 起建 parent 与索引，严禁回退到 Run 原始 `base_sha` 导致局部 `paths` 提交抹除上一轮改动。
- **I3（PR Gate 异步化）**：PR Gate 的长流程（推送、异构评审、Worker 返工、复审）由宿主后台作业持有并持久化阶段状态；HTTP 与 MCP `tools/call` 仅负责校验、落库建 Gate 并立即返回 ID。
- **I4（设备撤销即刻切断存量流）**：撤销已配对设备后，除拒绝新请求外，必须立即关闭该设备当前持有的全部存量 SSE 长连接。

### 从 `dsh-superagent-prototype` 吸收的 4 项本轮拼图
1. **单一权威 `Attention` 穷尽枚举**：在 Rust 后端用不带 `_` 分支的穷尽 `match` 统一定义“待人工决策”状态，前端 `Needs you`、桌面托盘与私密唤醒共用同一真相源。
2. **先存储，再执行**：将 Worker Turn 已成熟的“落库 + 后台推进 + 重启恢复”范式推广到 PR Gate（I3）。
3. **模型可见面一致性测试（`model-surface`）**：用自动化单测锁定“CLI Hub 的参数配置与工具白名单必须覆盖 `tools.rs` 声明的全部 Hub 工具”。
4. **结构化原因码 + `auto-merge` 三条纪律**：保留可读的结构化拒绝/失败原因直达 UI；将“非 agent 账号启用、`expectedHeadOid` 锁 SHA、推新修订前先撤回 auto-merge、`CLEAN` PR 直接按 `expectedHeadOid` 合并”全部写进宿主状态机。

---

## 1. 阶段一：单轮 P0 修复（按“验收证据可信度”分 3 批推进）

### Batch 1 — 执行底座、CLI Hub 权限与 Claude 沙箱（I2/B1 + B4 + B5 + `model-surface`）
- **目标提交信息**：`fix: preserve revision from_sha, fix cli hub tool permissions, and harden claude seatbelt`
- **任务明细**：
  1. **I2 / B1（返工 `from_sha` 丢改动）**：
     - `crates/polyclave-core/src/runs.rs`：`HostTools::commit_and_stage` 读取 `store.attempt_from_sha(scope.id)` 并传入 `MirrorManager::commit_and_stage`。
     - `crates/polyclave-core/src/stage.rs`：当 `staged == None` 且 `from_sha == Some(prior)` 时，从 `attempt.run.mirror` 拉取上一 Attempt ref、校验 OID 等于 `prior` 且 `base_sha` 为其祖先，将 `parent` 设为 `prior`（不再回退 `attempt.run.base`）。
     - **单测**：构造 Attempt 1 改动 `a.rs` 与 `b.rs`，Attempt 2 返工仅传 `paths: ["a.rs"]` 调 `commit_and_stage`，断言生成提交父节点为 Attempt 1 且 `b.rs` 改动完整保留。
  2. **B4 + `model-surface` 一致性测试**：
     - `crates/polyclave-hub/src/cli_provider.rs`：为 Codex CLI Hub 补齐 `mcp_servers.host.default_tools_approval_mode="approve"`；将 `"run_pr_gate"` 加入 `CENTRAL_TOOLS`。
     - **单测**：新增 `model_surface` 一致性测试，断言 Codex 与 Claude CLI Hub 的参数及工具白名单覆盖 `hub_api/tools.rs` 声明的 Hub 工具且具备非交互放行配置。
  3. **B5（拆分为 3 个独立子项修复）**：
     - **DNS 修复**：`crates/polyclave-adapters/src/claude.rs` SBPL 放行 `(remote unix-socket (path-literal "/private/var/run/mDNSResponder"))`。
     - **凭据读隔离**：`crates/polyclave-adapters/src/claude.rs` 显式 `(deny file-read* ...)` 屏蔽宿主敏感目录（`~/.ssh`、`~/.codex`、`~/.config/gh`、`~/.aws`、`~/.gnupg`、`~/.kube`、`~/.netrc`、`~/.git-credentials`）。
     - **认证与代理透传**：`crates/polyclave-adapters/src/lib.rs` `base_environment()` 补充代理变量（`HTTP_PROXY`、`HTTPS_PROXY`、`ALL_PROXY`、`NO_PROXY`），并允许宿主侧受控透传 `ANTHROPIC_API_KEY` / `CLAUDE_CODE_OAUTH_TOKEN` 给 Claude 进程（继续走 secretless 过滤）。
     - **单测**：macOS `sandbox-exec` 探针验证沙箱内域名解析可通且敏感目录拒读。
  4. **顺带 P1（执行诊断与重试预算）**：
     - `crates/polyclave-core/src/runner.rs`：保留 Worker `stderr` 尾部（经 token 脱敏）写入错误 `detail`（不再仅写未读 `Inbox` warning 并误导成 `retryable`）；`schedule_revision` 纳入自动纠正；`max_attempts` 默认提高到 `5`（`schema.sql` / 默认配置）。

### Batch 2 — PR Gate 异步化、CLEAN 合并、防重复武装与手机单页审批闭环（I3/B3 + I1/B7/B8 + B9 + 穷尽 `Attention`）
- **目标提交信息**：`fix: make pr gate async, handle clean merges, guard armed sha, and unify mobile approval`
- **任务明细**：
  1. **I3 / B3（先存后做异步 PR Gate）**：
     - `bins/polyclaved/src/hub_api/tools.rs` 与 `http.rs`：`run_pr_gate` 完成前置校验与落库建 Gate 后立即返回 `gate_id`，将 push / 异构评审 / Worker 返工 / 复审循环放进 `Hub` lifecycle 托管的后台任务；daemon 启动恢复时将滞留的 `reviewing` / `revise_requested` Gate 显式转为 `escalated` 并记录原因。
  2. **I1 / B7 & B8（CLEAN 合并 + `auto-merge` 纪律 + 防兄弟 Gate 换 head）**：
     - `crates/polyclave-hub/src/scm.rs`：读取 `mergeStateStatus` 并新增 `merge_pull_request(pr_node_id, expected_head_oid, SQUASH)`。
     - `crates/polyclave-hub/src/pr_gate.rs`：PR 处于 `CLEAN` / `HAS_HOOKS` / `UNSTABLE` 时直接调 `mergePullRequest`（带 `expectedHeadOid`）；遇到确定性远端拒绝时回退 `AwaitingHumanApproval`（不再卡死在 `approved`），并透传脱敏后的可读错误与 `PreflightReport.issues`。
     - `crates/polyclave-hub/src/pr_gate.rs` + `crates/polyclave-store/src/stage2.rs`：同 Run 存在 `auto_merge_armed` 或 `awaiting_human_approval` 的活跃 Gate 时拒绝新建/重跑兄弟 Gate；任何允许改动远端 head 的路径必先确认远端撤回 auto-merge；`scm_observer.rs` 观察到 `merged` 时校验 PR head 等于已批准 SHA。
  3. **B9 + 穷尽 `Attention` 单一真相源**：
     - Rust 后端新增不带 `_` 分支的穷尽 `match` 判定函数（覆盖 `RunStatus`、`PrGateStatus`、`CouncilStatus`），统一供 `crates/polyclave-hub/src/notify.rs`（纳入 `awaiting_human_approval`）、桌面托盘与前端 `Needs you` 消费。
     - `apps/polyclave-app/ui/src/views/inspector.ts`：在已 R1 核证（`done` + `agent`）的 Run 卡片增加 `Open PR Gate` 入口（可填 `repo_slug` / 选异构 Reviewer）；武装状态下隐藏 `Run PR Gate` 按钮；`PR Gates` 审批卡内嵌同 `(run_id, head_sha)` 的测试结果与 `receipt_not_done` 未决项、渲染 `pr_url` 外链，并在 diff 超过 400 行时明确提示截断与完整查看入口。
     - `apps/polyclave-app/ui/src/state/store.ts`：将 `awaiting_human_approval` 纳入 `attentionPrGates()`；补齐“存在待批准 Gate 时 `Needs you` 计数必 > 0 且与列表长度一致”的浏览器测试。
  4. **顺带 P1（重连、WKWebView 样式与文档）**：
     - `apps/polyclave-app/ui/src/app.ts`：封装循环 `startFollow()` 修复二次 401 失联，并在每次 SSE 重连时清空 `S.live`。
     - `apps/polyclave-app/src-tauri/tauri.conf.json`：对齐 `style-src 'self' 'unsafe-inline'`（或将设置页与进度条内联样式迁入 CSS 类）。
     - `README.md`：补全 `POLYCLAVE_TAILSCALE_ORIGIN` 与 GitHub Keychain 双账号配置说明。

### Batch 3 — 撤销设备切断 SSE、优雅关停、双安全阀与 PATH 兜底（I4/B10 + B2 + SV1/SV2 + B6）
- **目标提交信息**：`fix: close sse on device revoke, bound daemon shutdown, and seal evolution safety valves`
- **任务明细**：
  1. **I4 / B10（撤销设备立即断开存量 SSE）**：
     - `bins/polyclaved/src/auth.rs`：将已验证的 `device_id` 写入 request extensions。
     - `bins/polyclaved/src/routes.rs`：`/api/events` 持有 `device_id` 并订阅撤销广播，`revoke_device` 成功后立即关闭该设备所有存量 SSE 流。补单测：保持 SSE 打开后撤销设备，验证旧流立即结束。
  2. **B2（有界优雅关停 + 防跨进程重复结算）**：
     - `bins/polyclaved/src/main.rs`：将 `_lock` 上提至 `main` 并使用 `Runtime::shutdown_timeout`。
     - `bins/polyclaved/src/hub_api.rs`：`Hub::close()` 取消所有活跃 Worker Run 并在关闭 MCP 前有界等待。
     - `crates/polyclave-core/src/runner.rs`：`settle()` 的 `UPDATE turns` 加上 `AND status='running'` 守卫。
  3. **SV1（技能同步 symlink 隔离）**：
     - `crates/polyclave-adapters/src/lib.rs`：不再将整个 `skills` 目录做成指向用户全局目录的符号链接（改为在真实目录内按条目只读链接）。
     - `crates/polyclave-hub/src/evolution.rs`：`sync_skill_to_isolated_homes` 写入前强制校验 `skills_root` 非符号链接且 `canonical_real_path` 严格位于 `isolated-homes` 之内。
  4. **SV2（社区草稿脱敏加固）**：
     - `crates/polyclave-hub/src/evolution.rs`：`redact_text` 改为全文边界/子串扫描（拦截引号、反引号、`KEY=VAL`、`file://` 等内嵌本机路径、邮箱与凭据前缀）。
  5. **B6（Finder/Tauri PATH 兜底）**：
     - `crates/polyclave-adapters/src/lib.rs` / `crates/polyclave-os/src/path.rs`：解析 CLI 与构造 `base_environment()` 时补充常见 macOS 安装路径（`/opt/homebrew/bin`、`/usr/local/bin`、`~/.local/bin`、`~/.cargo/bin` 及 `POLYCLAVE_EXTRA_PATH`）。

### 每批统一验证门禁（硬要求）
1. Rust 全量检查：`cargo fmt --all -- --check && cargo clippy --locked --workspace --all-targets -- -D warnings && cargo test --locked --workspace`
2. 涉及 UI 或协议时追加：`git diff --exit-code -- apps/polyclave-app/ui/src/generated/proto.ts`、以及在 `apps/polyclave-app/ui` 下执行 `npx tsc --noEmit && npm run build && POLYCLAVE_BROWSER_CHANNEL=chrome npm run test:browser`
3. 推送前：`bash scripts/privacy-scan.sh` 必须输出 `privacy-scan: clean (origin/main..HEAD)`，并确认 GitHub Actions `macos-latest` 与 `windows-latest` 双平台 CI 全绿。

---

## 2. 阶段二：单故事真机端到端验收清单（合并四模型第 4 节假设）

在真实 Mac + 真实 `polyclaved` + `tailscale serve` + 真实 `codex`/`claude` CLI + 真实 GitHub 仓库 + 真实手机上逐项验证并把证据记入 `HANDOFF.md`：

1. **V1 · 真实 CLI 参数、认证、MCP 握手与沙箱探针**：
   - 验证本机 `codex` 与 `claude` 接受当前参数并成功连入本机 MCP 完成 `commit_and_stage` 与 `submit_result`。
   - 验证 Claude Seatbelt 在可正常解析域名与完成认证的同时，无法读取宿主敏感凭据目录（`~/.ssh` 等）。
2. **V2 · 中途重启与两端状态一致性**：
   - 在 Worker 写码途中分别测试 `kill -9` 与 `Ctrl-C`/`SIGTERM` 重启 `polyclaved`：确认 3 秒内无孤儿进程残留、Run 准确结算、桌面端（含连续两次重启）与手机端均无 `S.live` 旧流式文本残留。
3. **V3 · 真实 GitHub 仓库、双账号与返工 `from_sha`**：
   - 验证真实私有/测试仓库 `preflight`、双账号（`bot_token` + `reviewer_token`）互不相同、异构 Reviewer 提交 `REQUEST_CHANGES` → Attempt B 返工仅改部分文件时完整保留 Attempt A 其余改动 → 复审 `APPROVE`。
4. **V4 · `CLEAN` PR 合并与 `auto-merge` 武装/撤回**：
   - 验证 CI 已绿的 `CLEAN` PR 在手机点击批准后直接按 `expectedHeadOid` 合并；
   - 验证 CI 未完成的 PR 批准后武装 auto-merge，且武装期间 UI 禁止重跑 Gate，全局 Pause 可正确撤回远端授权。
5. **V5 · Tailscale Serve、375px 手机审批与设备撤销**：
   - 验证 `POLYCLAVE_TAILSCALE_ORIGIN` + `tailscale serve --bg 4310` 下扫码配对成功、`Tailscale-User-Login` 注入、刷新免重扫；
   - 在 375px 真机视口从 `Needs you` 直达同一张审批卡，同屏查看精确 SHA 的 diff、同 SHA CI 测试与 `not_done` 未决项并完成批准；
   - 从桌面端撤销另一台保持 SSE 打开的已配对设备，确认其存量 SSE 立即断开且不再收到新消息。

---

## 3. 阶段三：真机验收通过后的下一阶段待办（本轮明确不做）

- **角色级模型与推理等级预设 + 三级审查模式**：在 Settings 中为各 Agent / 角色（Hub、Worker、Reviewer、盲审席位）预先绑定默认模型与推理等级（reasoning effort / thinking budget），派工与发起盲审时自动生效；内置“日常单异构 / 省 Token 单轮多模型盲审 / 里程碑多轮交叉互评”三档审查模式。
- 借鉴 `dsh-superagent-prototype` 的 4 项进阶工程能力：**`SETUP.md` AI 引导式配置** + **面向外部读者的编号短文文档（`000`–`005`）**、**真实启动环境预检（Setup Problems 面板）与只读数据库快照回放**、**PR Gate `dry-run` / `Observe` 演习模式**、**人的优先槽位 + 失败三次进系统问题降噪**。
- Spec Decision 15（`raise_plan_issue` 4 段式决策卡、2 轮返工熔断）及此前暂停的 Mesh、`host_run`、CUA、自进化/社区蒸馏扩展。
- **明确不抄的红线**：不引入团队多源准入矩阵与飞书/第三方 IM 云入口、不把只读 Hub 缩成薄 Router、不用提示词代替宿主安全边界、不采用“tree 不变允许改写历史”的审批模型、不复用脏目录跳过 Attempt 隔离。
