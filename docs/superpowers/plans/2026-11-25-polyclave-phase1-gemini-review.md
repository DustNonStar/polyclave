# Polyclave 阶段一（Revision 2）与 v4 设计开工前评审

- **评审基线**：`main` 分支提交 `93df7b0`（2026-11-25）
- **评审对象**：
  - `docs/superpowers/plans/2026-11-25-polyclave-phase1.md`（Revision 2）
  - `docs/superpowers/specs/2026-11-25-agent-console-design-v4.md`
  - `docs/superpowers/plans/2026-11-25-polyclave-phase1-gate-a-revisions.md`
  - `docs/superpowers/research/2026-11-24-t3-probe-results.md`、`2026-11-25-orca-comparison.md` 及 `probes/*.mjs`
- **实机核验环境**：macOS（Darwin / BSD `ps`、`pgrep`、`sysctl`）、Node.js（原生 TypeScript 类型擦除、`node:sqlite`、`node:child_process` 第 4 管道）、`typescript@6.0.3`、`@types/node@22.20.0`、`vitest@4.1.8`、`zod@4.4.3`、`codex-cli 0.172.0`、`git`

---

## 一、GPT 评审 8 项阻塞问题的修复核查（评估项 1）

| GPT 阻塞项 | 结论 | 计划中的证据与剩余缺口 |
|---|---|---|
| **1 · 工具进程脱离外层组** | **部分修复** | **已落实**：Global Constraints 第 26–27 行、Task 3 第 251/262 行、Task 7 第 478–479 行及 Task 8 第 518 行引入 `bootTime`、`writerCheck` 与会话冻结，实证覆盖了宿主 `SIGKILL` 和运行任务期间用户取消。<br>**剩余缺口**：① 普通聊天轮次同样以 `workspace-write` + `gitCommonDir(session.cwd)` 运行 Codex（第 22、363 行），取消聊天轮次时标了 `writerCheck = 'possible'`（第 366 行），第 27 行却规定不冻结会话且第 367 行立即调用 `drainQueues()` 启动下一轮；② 二次信号 `runner.kill()`（第 358、551 行）将轮次写成 `interrupted` + `writerCheck = 'possible'` 却未冻结聊天会话，同次开机重启时 `recover()` 只扫 `running` 轮次（第 476 行），直接漏掉该会话；③ `Store.endTurn`（第 124 行）缺少 `WHERE status = 'running'` 保护，子进程退出回调会把已写入的 `writerCheck = 'possible'` 覆盖回 `'excluded'`。详见 **[B2]**。 |
| **2 · PID 复用及先启动后登记** | **部分修复** | **已落实**：第 24–25 行引入 16 位十六进制 `hostInstanceId` 判定轮次归属（Task 7 第 490 行测试同 PID 不同实例），Task 3 第 228–239 行与 Task 5 第 362 行加入 fd 3 `ready`/`go` 握手，先提交 `authorizeTurn` 再发 `go`。<br>**剩余缺口**：① `Supervised.child: ChildProcessWithoutNullStreams` 在 `stdio: ['ignore', 'pipe', 'pipe', 'pipe']` 下触发 `tsc` 编译错误 `TS2322`；② `supervisor.ts`（第 239 行）在直接子进程收到 `SIGTERM` 先退出时若立即退出，会提前销毁 2 秒 `SIGKILL` 定时器导致同组孙进程存活，反之若死等 2 秒再发 `-pgid` `SIGKILL` 又会杀死组长自身；③ `ps`/`pgrep`/`sysctl` 在 macOS 上存在时区语言环境敏感、`PID > 99999` 报错、组长退出瞬间 `groupLiveness` 误判 `unverifiable` 以及 `acquireHostLock` 非原子抢锁竞态。详见 **[B6]、[M2]**。 |
| **3 · 推送和核对指向不同裸库** | **已修复** | R3（第 32 行）与 Task 4 `preflightTarget`（第 294 行）同时核对 `git remote get-url origin` 与 `git remote get-url --push --all origin`，要求唯一且 `realpath` 一致的本地裸库并开启 `receive.denyDeletes=true`；`run.remote` 在受理时固化，Task 6 `runBrief`（第 414 行）与 Task 4 `checkRunBranch`（第 298 行）均直接针对该绝对路径读写完整 refspec（建议补充 `receive.denyNonFastForwards=true`，见 **[L2]**）。 |
| **4 · 共用 checkout、基线漂移** | **部分修复** | **已落实**：每个 `RunRow` 拥有独立 `baseSha`、`branch`、`worktreePath`（`<stateDir>/worktrees/<runId>`）与独立 `agentSessionId`（第 28、106、363 行），`checkRunBranch` 校验 `baseSha` 祖先、非空区间与真实 Git trailer（第 298、323 行）。<br>**剩余缺口**：`prepareWorktree` 只在新建 `Runs.create` 时调用一次（第 408 行），`Runs.start` 与 `Runs.retry`（第 433 行）均未调用；若宿主在 `acceptRun` 提交后、`prepareWorktree` 完成前被强杀，重启后执行 `retry` 会因 worktree 目录不存在直接报错甚至锁死会话；此外复用已有 worktree 时未校验仓库归属与 `baseSha` 祖先，非冲突 git 错误还会把记录卡死在 `pending`。详见 **[B4]**。 |
| **5 · 已受理任务搁置或重复启动** | **部分修复** | **已落实**：`(session_id, request_id)` 唯一索引、`(session_id) WHERE status IN ('pending','running')` 占用索引、排队消息在 `claimTurn` 事务内原子绑定（第 146–150 行），以及 `recover()` 处理 `pending` 任务（第 480、492 行）。<br>**剩余缺口**：① `Runs.retry` 在异步 `checkRunBranch` 前未占锁，且 `claimTurn` 允许 `run.status === 'running'`，两个并发 `retry` 可在首轮与纠正轮间隙穿插启动第二个执行循环或突破 3 次上限（**[B1]**）；② `Runs.create` 第 408 行“先按 `requestId` 重放返回、跳过 preflight”因 `Store` 缺少独立校验方法，按字面实现会绕过输入变更时的 `ConflictError`（**[B5]**）；③ 排队消息在 `prepared` 阶段（agent 未启动）遇崩溃会被永久清除 `queued` 标志而静默丢失（**[M4]**）。 |
| **6 · 退出、取消、恢复的裁决分叉** | **部分修复** | **已落实**：正常退出、非零退出、取消、恢复、手动重试统一走 Task 4 `classifyRun`（第 302–312 行），已推未交立即转 `needs_review`（0 次纠正，第 443 行），已推且已接受后非零退出仍判 `done`（第 442 行）。<br>**剩余缺口**：① 关机期间（`runner.shutdown()` 关闭新派发后）若在途首轮以退出码 0 结束且未推送，Task 6 结算表（第 428 行）缺少关机分支，仍会递增 `corrections` 并调用 `runTurn`（**[M1]**）；② 人工裁决入口 `Runs.resolve({ decision: 'accept' })` 不校验用户看到的 `observedSha` 与 `baseSha` 祖先，且 `classifyRun` 第 1 行漏写 `observedSha` 取值（**[B3]**）。 |
| **7 · MCP 查令牌与写收据之间存在竞态** | **已修复** | Task 1 `settleResult`（第 140、149 行）与 `UNIQUE (run_id) WHERE accepted = 1`（第 150 行）在远端 Git 检查完成后用短事务重验 `running` + `authorized` + `hostInstanceId` + `runId`，取消抢先提交则返回 `turn_not_active` → HTTP `401`（第 314、331–333 行）。但拒收分支（`addRejectedResult`）在聊天轮次误调或 `checkRunBranch` 抛错时仍有异常未捕获缺陷，归入 **[B5]、[M3]**。 |
| **8 · fake、提示词与 harness 合同冲突** | **部分修复** | **已落实**：`fake-codex` 启动即记 `prompt_started`、拆分 `commit`/`push`/`submitPushed`/`detachedWriter`、不记 token（第 245–252 行）；`makeRepo` 用 `post-receive` 独立记 `refLog`（第 243 行）；`host-harness.ts` 增加 `serve` 常驻模式（第 511 行）。<br>**剩余缺口**：① `fake-codex` 依赖从提示词提取 `Task r…` 和 `git push …` 命令，但 Task 6 `correctionText`（第 419 行）写成小写 `task <runId>` 且不含任何 `git push` 命令，导致第 445 行纠正成功测试必败（**[B5]**）；② Task 8 第 515、521 行的“首轮前被杀”与“放行前被杀”发生在 `fake-codex` 启动前，`host-harness.ts` 无对应屏障也无同宿主并发命令（**[B7]**）。 |

---

## 二、分等级问题清单（覆盖评估项 2–7）

### 阻塞（Blocking）

#### [B1] 并发 `Runs.retry` 存在异步检查窗口竞态，且 `Store.claimTurn` 允许在 `running` 状态下穿插启动第二个任务循环
- **位置**：`docs/superpowers/plans/2026-11-25-polyclave-phase1.md` Task 1（第 122、147 行）、Task 6（第 428、433 行）、Task 8（第 515 行）。
- **问题**：`Runs.retry` 先读取 `run.status === 'interrupted'` 与 `retries < 3`，随后在数据库事务外异步执行 `await checkRunBranch(...)`，最后才调用 `runner.runTurn` → `store.claimTurn`。而 `claimTurn`（第 147 行）为了兼顾首轮、重试和纠正轮，把允许的前置状态宽泛定义为 `pending | running | interrupted`，且仅在 `interrupted → running` 时递增 `retries`（内部也未校验 `retries < 3`）。
- **具体场景**：
  1. 任务 `r1` 处于 `interrupted`、`retries = 2`。两个并发请求同时调用 `Runs.retry(r1)`（对应 Task 8 第 1 行 `two concurrent retries start at most one turn`），双方都读到 `interrupted` 且 `retries = 2 < 3`，双双进入 `await checkRunBranch(...)`。
  2. 请求 A 先完成检查并进入 `claimTurn`，将 `r1` 置为 `running`、`retries = 3`，启动第 1 轮。该轮很快以退出码 0 结束但未推送，请求 A 准备发送第 1 条 `correctionText`。就在第 1 轮 `endTurn` 已提交、纠正轮 `claimTurn` 尚未执行的间隙，会话上没有 `running` 轮次，而 `r1.status` 为 `'running'`。
  3. 请求 B 此时完成 `checkRunBranch(...)` 并调用 `runTurn` → `claimTurn`：会话无运行中轮次（不触发 `BusyError`），`r1.status === 'running'`（满足第 147 行条件，且因不是从 `interrupted` 转换，连 `retries` 都不递增）。请求 B 成功启动**第二个并行的 `Runs` 循环**，两个循环在同一 worktree 和同一 Codex 线程上并发跑轮次、互相踩分支与纠正计数。若第 1 轮因远端读取失败退回 `interrupted`，请求 B 还会越过 `retries < 3` 跑出第 4 次重试。
- **修改建议**：
  1. 为 `Store.claimTurn` 增加显式认领意图参数 `runMode?: 'start' | 'correction' | 'retry'`：`start` 要求 `run.status = 'pending'`；`retry` 在同一 `BEGIN IMMEDIATE` 事务中要求 `run.status = 'interrupted' AND run.retries < 3`，原子置为 `running`、`retries = retries + 1`、`corrections = 0`；`correction` 要求调用方传入上一轮刚结束的 `previousTurnId`（匹配该 run 的最新 `turn.id`）且 `run.status = 'running'`。
  2. 在 `Runs` 内存中按 `runId` 维护一个执行互斥集（`inFlightRuns: Set<string>`），在 `retry()` 进入异步 `checkRunBranch` 前即拦截同进程并发重入。

---

#### [B2] 聊天轮次取消、`runner.kill()` 二次信号与无条件 `Store.endTurn` 存在写入者漏冻结与证据覆盖漏洞
- **位置**：`docs/superpowers/plans/2026-11-25-polyclave-phase1.md` Global Constraints（第 22、26–27 行）、Task 1（第 124 行）、Task 5（第 358、363、366–367 行）、Task 7（第 476–480 行）、Task 9（第 551 行）；对照 `2026-11-25-polyclave-phase1-gate-a-revisions.md` 第 82 行。
- **问题**：计划在三处破坏了“只要已放行的 agent 被硬杀且同次开机内未排除脱离进程组的写入者，就不得在该会话启动新轮次”的不变量：
  1. **聊天轮次被取消不冻结且立即排空队列**：普通聊天同样使用 `sandbox_mode="workspace-write"` 和 `writableRoots=[gitCommonDir(session.cwd)]`（第 22、363 行），Codex 执行的构建、脚本或 `git` 命令同样经 `POSIX_SPAWN_SETSID` 脱离外层进程组。第 366 行把取消记为 `writerCheck = 'possible'`，第 27 行却规定取消聊天不冻结，且第 367 行在聊天结束时立即调用 `drainQueues()`。
  2. **二次信号 `runner.kill()` 写 `interrupted` 但不冻结聊天会话**：第 358、551 行规定第二次 `SIGINT`/`SIGTERM` 调用 `runner.kill()` 将运行中轮次落库为 `interrupted`、`endReason = 'host_lost'`、`writerCheck = 'possible'` 后退出。由于 `status` 已不再是 `'running'`，同次开机重启时 `recover()` 第 1 步（只查 `running` 轮次，第 476 行）和第 2 步（只查 `runs`，第 480 行）都会完全略过该聊天会话。
  3. **`Store.endTurn` 无状态前置条件**：第 124 行 `endTurn` 未限定 `WHERE status = 'running'`。
- **具体场景**：
  - **场景 A（取消或 `kill()` 后被子进程退出回调覆盖）**：用户对运行中的任务点取消（或连按两次 `Ctrl-C` 触发 `runner.kill()`）。为使 MCP 令牌立即失效（满足 Task 4 第 331 行），`cancel()`/`kill()` 先调用 `endTurn(..., { writerCheck: 'possible' })`，再调 `killGroup()`。进程组被信号杀死后，`runTurn` 监听到子进程非零退出（第 366 行 `exit ≠ 0 → failed, exit, writer excluded`），再次调用 `endTurn`，将数据库里的 `'cancelled'`/`'interrupted'` 与 `writerCheck = 'possible'` 覆盖为 `'failed'` 与 `writerCheck = 'excluded'`，导致 `Runs` 把任务判成普通 `failed`、会话不冻结，脱离组的后台工具继续写仓库和推送。
  - **场景 B（连按两次 `Ctrl-C` 重启后串写）**：聊天会话正在跑耗时脚本，用户连按两次 `Ctrl-C` 重启宿主。重启后 `recover()` 因该轮已是 `interrupted` 而跳过冻结，启动末尾的 `runner.drainQueues()` 立即在同一 `session.cwd` 和同一 Codex 线程上并发拉起排队消息。
- **修改建议**：
  1. `Store.endTurn` 改为只在 `WHERE id = ? AND status = 'running'` 时更新并返回 `boolean`（CAS 语义），一旦写入终态永不被后续退出回调覆盖。
  2. 恢复 GPT 修订稿第 82 行的统一规则：任何已 `authorized` 轮次因硬杀（包括聊天轮次 `cancel`）产生 `writerCheck = 'possible'` 时，一律在同一事务中调用 `freezeSession`（取消时用户就在界面前，确认安全后点一次 Unfreeze 即可）。
  3. 删去 `runner.kill()` 改写数据库状态的旁路（见 **[L3]**），或在 `recover()` 中增加会话级检查：凡未冻结会话的最后一轮满足 `writerCheck = 'possible' AND bootTime = currentBootTime` 且其后无 `Unfrozen by user:` 消息的，一律补齐冻结。

---

#### [B3] `Runs.resolve({ decision: 'accept' })` 未绑定用户观测到的 SHA，且 `classifyRun` 遗漏 `observedSha` 赋值规则，可把未审代码标为 `done`
- **位置**：`docs/superpowers/plans/2026-11-25-polyclave-phase1.md` Task 4（第 287、304–310 行）、Task 6（第 397、435 行）、Task 9（第 547 行）、Task 10（第 574 行）；对照 `docs/superpowers/specs/2026-11-25-agent-console-design-v4.md` §7.5 第 285 行。
- **问题**：
  1. v4 设计稿 §7.5 第 285 行规定用户对 `needs_review` 选择接受时“以当时看到的提交为准”，但计划中 `Resolution`（第 397 行）没有 `sha` 字段，第 435 行规定 `accept → reads the tip; requires a tip; done, resultSha = tip, resultSource 'user'`——既不核对当前远端 tip 是否等于用户界面上看到的 `observedSha`，也不核对 `baseSha` 是否为该 tip 的严格祖先。
  2. Task 4 `classifyRun` 表（第 304–310 行）共有 5 行返回 `needs_review`，却只有第 4 行注明了 `(observedSha = tip)`；最常触发的第 1 行（`writer = possible`，同次开机崩溃或取消）及第 5、6 行均未说明 `observedSha` 的取值。
- **具体场景**：
  1. 任务首轮已推送合格提交 `X`，随后宿主被 `SIGKILL`。重启后因同次开机 `writer = possible` 命中 `classifyRun` 第 1 行：若实现者按字面只给第 4 行填 `observedSha`，此时 `run.observedSha` 为 `null`，前端（Task 10 第 574 行）根本展示不出提交 `X`。
  2. 即便记录了 `observedSha = X`，用户检查 `X` 后在网页点击 Accept；恰在点击前一瞬，上一轮残留的脱离进程组工具完成了第二次推送 `Y`（半成品或错误分支）。`Runs.resolve` 直接读取最新 tip `Y` 并将 `resultSha = Y` 标为 `done`，违反“绝不把未完成或未经确认的结果标为完成”的核心目标。
- **修改建议**：
  1. 在 Task 4 明确：所有 `needs_review` 裁决行的 `observedSha` 均取 `remote.state === 'present' ? remote.sha : null`。
  2. 将 `Resolution` 改为 `{ decision: 'accept'; sha: string; note: string } | { decision: 'fail' | 'allow_retry'; note: string }`（前端传入当前展示的 `observedSha`）。`Runs.resolve` 处理 `accept` 时重新在裸库核对：`remote` 必须 `present`、`remote.sha === r.sha`、且 `baseSha` 是 `remote.sha` 的祖先且区间非空；若远端 tip 已变为其他值，更新 `run.observedSha = remote.sha` 并抛出 `StaleStateError`（HTTP `409`），让用户审阅新 SHA 后再决定。

---

#### [B4] `prepareWorktree` 仅在首次 `Runs.create` 调用，首轮前崩溃后 `retry` 因目录缺失失败，且异常路径可永久锁死会话
- **位置**：`docs/superpowers/plans/2026-11-25-polyclave-phase1.md` Task 4（第 296 行）、Task 5（第 362–363 行）、Task 6（第 408、433 行）、Task 7（第 480 行）、Task 8（第 515 行）。
- **问题**：
  1. `Runs.create`（第 408 行）先执行 `acceptRun`（提交事务写入 `pending`，由唯一索引锁死该会话），再执行 `prepareWorktree`；而 `Runs.start`、`Runs.retry`（第 433 行）以及同 `requestId` 重放路径都不调用 `prepareWorktree`。
  2. `Runs.create` 仅捕获 `WorktreeConflictError` 转为 `needs_review`；`prepareWorktree` 若因其他 Git/IO 错误失败，记录会永久留在 `pending`。
  3. `TurnRunner.runTurn`（第 362–363 行）先调用 `claimTurn` 写入 `status = 'running'`，再计算 `gitCommonDir(worktreePath)` 和 `spawnSupervised`，未规定 `authorizeTurn` 前异常的回滚清理。
  4. `prepareWorktree`（第 296 行）复用已有目录时只检查“路径和分支存在且当前切在该分支上”，未按 GPT 修订稿（Task 6 第 76 行）核对所属主仓库与 `baseSha` 祖先关系。
- **具体场景**：
  - 在 Task 8 第 1 行用例（`kill before the first turn`）中，宿主在 `acceptRun` 落库后、`prepareWorktree` 创建 `<stateDir>/worktrees/<runId>` 之前被 `SIGKILL`。重启后 `recover()` 将该 `pending` 任务置为 `interrupted`（`safe to retry`）。用户执行 `retry`：`Runs.retry` 直接调 `runTurn`，`claimTurn` 先把 run 和新 turn 都置为 `running`，随后 `gitCommonDir(run.worktreePath)` 因目录不存在抛错退出——不仅 `retry` 必败，数据库中残留的 `running` turn 还会把整个会话永久卡在 `BusyError`。
  - 同理，若 `Runs.create` 中 `git fetch` 报错（非 `WorktreeConflictError`），留下的 `pending` 行在 UI（Task 10）中既无 Retry 也无 Resolve 按钮，会话将被永久阻塞而再也建不了新任务或发送聊天。
- **修改建议**：
  1. 在 `Runs.start` 和 `Runs.retry` 启动 turn 前都调用幂等的 `prepareWorktree`；`prepareWorktree` 复用已有目录时补充核对 `gitCommonDir(path) === gitCommonDir(cwd)` 且 `git -C <path> merge-base --is-ancestor <baseSha> HEAD`，不满足则抛 `WorktreeConflictError`。
  2. `prepareWorktree` 抛出 `WorktreeConflictError` 时将 run 置为 `needs_review` 并调 `runner.drainQueues()`；抛出其他错误时将 run 置为 `interrupted`（`detail = 'worktree preparation failed: <msg>'`）并调 `runner.drainQueues()`。
  3. `TurnRunner.runTurn` 在 `claimTurn` 之后用 `try/catch` 包裹启动过程：凡在 `authorizeTurn` 返回 `true` 之前抛错的，一律调用 `abort()`（若已启动 supervisor）并执行 `endTurn(turn.id, 'interrupted', { exitCode: null, endReason: 'never_started', writerCheck: 'excluded' })`。

---

#### [B5] 跨任务契约断裂：`fake-codex` 跑不通纠正轮、解冻备注吞掉聊天中断提示、`Runs.create` 重放绕过冲突检查、`Store` 缺 4 个必需查询方法
- **位置**：`docs/superpowers/plans/2026-11-25-polyclave-phase1.md` Task 1（第 107、113–142 行）、Task 3（第 247–249 行）、Task 4（第 314 行）、Task 5（第 365 行）、Task 6（第 408、418–420、428、445 行）、Task 7（第 480 行）、Task 9（第 540、557 行）、Task 11（第 590 行）。
- **问题与具体场景**：按计划逐任务实现时，以下 5 处接口/文案不匹配会直接导致验收测试失败或实现者被迫猜接口：
  1. **Task 6 第 445 行 `completes after one correction` 必败**：Task 3 第 247–248 行规定 `fake-codex` 的 `{ commit }` 从提示词的 `Task r…`（大写 `T`）提取 `runId`，`{ push: true }` “运行在提示词中找到的 push 命令”。但 Task 6 第 419 行的 `correctionText` 写的是小写 `task <runId>`，且全文只有 `Finish the task, push, then call submit_result…`，根本没有 `git push <remote> refs/heads/<branch>:refs/heads/<branch>`。第 2 轮收到 `correctionText` 时，`fake-codex` 既解析不出推送命令，也匹配不到大写 `Task r…`。
  2. **Task 11 第 590 行聊天强杀恢复测试必败**：Task 5 第 365 行规定仅当上一轮聊天为 `interrupted` 且其后“没有 `host` 消息（`no host note followed it`）”时才添加中断提示。但同次开机聊天在 `thread.started` 后被强杀，恢复时会冻结会话；发下一条聊天前必须先调 `POST /api/sessions/:id/unfreeze`（Task 9 第 540 行），而该接口恰好写入一条 `source = 'host'` 消息 `Unfrozen by user: <note>`。下一轮聊天检测到这条 `host` 消息后不再注入中断提示，测试断言直接失败。
  3. **Task 9 第 557 行 `replays a run request id and rejects a changed one` 矛盾**：Task 6 第 408 行要求 `Runs.create` 先按 `requestId` 重放返回（不跑 preflight），但 `Store`（Task 1）没有按 `(sessionId, requestId)` 查询并校验输入一致性的独立方法，校验逻辑被绑在需要 `remote` 和 `baseSha`（来自 preflight）的 `acceptRun` 里。若先查 `listRuns()` 直接返回已有记录，修改了 `instructions` 的同键请求会返回 `200` 而非 `409 ConflictError`；若先跑 preflight 再调 `acceptRun`，又违反第 408 行且在会话繁忙/冻结时连已完成的请求都重放不了。
  4. **`Store` 缺少 `lastChatTurn` 与 `lastRunTurn`**：Task 1 只有按会话查最后一轮的 `lastTurn(sessionId)`。Task 5 第 365 行需要查“该会话最后一轮聊天”（否则失败或中断的 run 轮次会污染聊天线程的中断提示判断），Task 7 第 480 行需要查“该 run 的最后一轮”（否则上一轮被取消的聊天留下的 `writerCheck = 'possible'` 会把刚受理、从未跑过轮次的 `pending` run 误判为 `needs_review`）。
  5. **`Store` 写拒收结果不可读且聊天误调 `submit_result` 无合法 `runId`**：Task 6 `correctionText(run, reason)` 需要填入最近一次拒收原因 `<reason>`，但 `Store` 只有 `addRejectedResult`（第 139 行），没有任何读取拒收记录的方法；同时 Task 4 第 314 行规定 schema 错误时调用 `addRejectedResult({ runId, ... })`，若聊天轮次（`turn.runId === null`）误调 `submit_result({})`，既无 `turn.runId` 也无合法 `args.run_id` 可写入非空的 `ResultRow.runId: string`。
- **修改建议**：
  1. 将 `correctionText` 改为：`Task <runId>: the host has no accepted result yet (<reason>). Nothing has been pushed for it. Finish the task, push with: git push <remote> refs/heads/<branch>:refs/heads/<branch> , then call submit_result with {"run_id": "<runId>", "branch": "<branch>", "sha": "<pushed commit sha>"}.`
  2. 将 Task 5 第 365 行的判据改为：该会话最后一轮聊天（`runId === null`）状态为 `interrupted`（且 `endReason !== 'never_started'`），并且在该轮之后尚未写入过以 `Note from the host: your previous turn was interrupted.` 开头的 `host` 消息。
  3. 在 Task 1 `Store` 中补齐 4 个方法：`replayRun(i: { sessionId: string; requestId: string; instructions: string; base: string }): RunRow | undefined`（命中但归一化输入不同抛 `ConflictError`）、`lastChatTurn(sessionId: string): TurnRow | undefined`、`lastRunTurn(runId: string): TurnRow | undefined`、`lastRejectedError(runId: string): string | null`（无记录时 Task 6 `<reason>` 取 `'no result submitted'`）。
  4. 在 Task 4 `submit_result` 中：通过 `turnByToken` 拿到 `turn` 后先判断 `turn.runId === null`，若是则直接返回 `Rejected: this session has no active task`（不调用 `addRejectedResult`）；其余校验失败调用 `addRejectedResult` 时一律使用 `turn.runId` 作为 `runId`。

---

#### [B6] Task 3 `Supervised.child` 类型通不过 `tsc --noEmit`，且 `supervisor.ts` 升级清理、fd 3 关闭与管道继承存在挂起/漏杀缺陷
- **位置**：`docs/superpowers/plans/2026-11-25-polyclave-phase1.md` Task 1（第 19、152 行）、Task 3（第 228–230、239、251、259 行）。
- **问题**：
  1. **编译失败（实测复现）**：在计划锁定的 `typescript@6.0.3` + `@types/node@22.20.0` + `strict: true` 下，`spawn(..., { detached: true, stdio: ['ignore', 'pipe', 'pipe', 'pipe'] })` 返回类型为 `ChildProcess`（其 `stdin` 为 `Writable | null`），赋给第 228 行的 `ChildProcessWithoutNullStreams`（要求 `stdin: Writable`）时运行 `tsc --noEmit` 直接报 `TS2322`。
  2. **2 秒 `SIGKILL` 升级与进程退出的矛盾**：第 239 行要求先挂 2 秒 `SIGKILL` 定时器再向进程组发 `SIGTERM`，同时规定“以 agent 的退出码退出”。若直接子进程（如外层 shell）收到 `SIGTERM` 在几毫秒内退出，而同组孙进程捕获并忽略了 `SIGTERM`：一旦 `supervisor.ts` 随直接子进程退出而立即结束，**Node 进程终止会导致 2 秒定时器一并消失**，孙进程永远不会被 `SIGKILL`（Task 3 第 259 行测试失败）；反之若无条件等满 2 秒再退，不仅每次正常取消都平白卡顿 2 秒，到点执行 `process.kill(-process.pid, 'SIGKILL')` 还会将组长 `supervisor.ts` 自己用不可捕获的 `SIGKILL` 杀死，丢失退出码。
  3. **fd 3 与 `stdout`/`stderr` 管道生命周期漏收口**：`supervisor.ts` 收到 `go\n` 后若未销毁 fd 3 Socket，只要父进程未关 fd 3，agent 退出后 `supervisor.ts` 仍不会退出；同时 `fake-codex` 的 `detachedWriter`（第 251 行）若未指定 `stdio: 'ignore'`，会继承 `stdout`/`stderr` 管道写端，导致宿主等待 `child.on('close')` 一直阻塞到测试屏障释放后。
- **修改建议**：
  1. 将 `Supervised` 接口改为 `child: ChildProcessByStdio<null, Readable, Readable>`（从 `node:child_process` 与 `node:stream` 导入类型）。
  2. 明确 fd 3 协议收口：宿主 `go()` 写入 `go\n` 后立即 `end()` 控制流；`supervisor.ts` 用 `new net.Socket({ fd: 3, readable: true, writable: true })` 读到 `go\n` 后立即 `destroy()` 该 Socket。
  3. 明确 `supervisor.ts` 终止状态机：触发终止（父进程丢失或收到信号）时挂 2 秒定时器并向 `-process.pid` 发 `SIGTERM`，同时每 50 ms 检查同组是否还存在除 `process.pid` 之外的成员；一旦直接子进程已退出且组内无其他成员，立即清掉定时器并 `process.exit(agentCode)`；若满 2 秒仍有其他成员存活，向 `-process.pid` 发送 `SIGKILL`。
  4. 明确 `fake-codex` 的 `detachedWriter` 必须传 `detached: true, stdio: 'ignore'` 并调用 `unref()`；`TurnRunner` 在 `cancel()`/`kill()` 等待子进程 `exit` 后主动销毁 `stdout`/`stderr` 流，不无限等待脱离进程组的子孙关闭管道。

---

#### [B7] Task 8 里程碑测试缺少“首轮前”与“放行前”的确定性屏障及同宿主并发请求通道
- **位置**：`docs/superpowers/plans/2026-11-25-polyclave-phase1.md` Gate A（第 57 行）、Task 8（第 508–525 行，尤其第 509、513、515、521 行）。
- **问题**：第 513 行要求 `Barriers, not sleeps, decide timing; timeouts only fail tests`，Gate A 要求连续运行 3 次零偶发失败。然而 `FAKE_CODEX_SCRIPT` 中的 `{ wait }` 屏障只能在 `authorizeTurn` 落库并发出 `go()` **之后**由 `fake-codex` 执行：
  1. **第 515 行（`same request twice, lost response, and a kill before the first turn`）**：`host-harness.ts` 启动即调 `acquireHostLock`，同时起两个 `host-harness.ts start-run` 第二个会在抢锁阶段直接退出，测不到同一宿主内的并发重复请求；且 `start-run` 把 `Runs.create` 和 `Runs.start` 串在一起跑完才输出 JSON，没有任何屏障能在“`acceptRun` 已提交但 `claimTurn` 尚未执行”处停住宿主供父进程 `SIGKILL`。
  2. **第 521 行（`killed before authorization`）**：`claimTurn → spawnSupervised → authorizeTurn` 全在宿主内部数十毫秒内完成，其间 `fake-codex` 尚未启动，没有任何非 `sleep` 手段能稳定把宿主杀在 `phase = 'prepared'` 状态。
- **具体场景**：实现者在写 `tests/milestone.spec.ts` 时，要么违反第 513 行偷偷用 `setTimeout(..., 15)` 撞运气（在负载波动下连续跑 3 次极易偶发杀在 `acceptRun` 之前或 `authorizeTurn` 之后而挂掉 Gate A），要么绕过 `host-harness.ts` 直接手工往 SQLite 插假数据（失去端到端验证宿主真实代码路径的意义）。
- **修改建议**：
  1. 扩展 `host-harness.ts` 的 `serve` 模式：通过 stdin 接收 JSON 行命令（`create-run`、`start-run`、`retry`、`resolve`、`unfreeze`）并在 stdout 返回带请求 `id` 的 JSON 响应，使父测试能在同一个持锁宿主进程上确定性发起并发相同 `requestId`、并发 `retry` 等操作。
  2. 支持仅供测试使用的环境变量屏障（如 `POLYCLAVE_BARRIER_AFTER_ACCEPT` 与 `POLYCLAVE_BARRIER_BEFORE_AUTHORIZE`），设置时宿主在 `acceptRun` 返回后、`authorizeTurn` 调用前各自调用文件 `barrier`（写出 `reached` 标记并等待父测试写出 `release` 或直接 `SIGKILL`）。

---

#### [B8] macOS + Codex CLI 0.172.0 实机阻塞：Seatbelt 拒收含符号链接分量的路径（`/var/folders`、`/tmp`）及相对 `.git`，且 `runBrief` 未写明 Git trailer 空行要求
- **位置**：`docs/superpowers/plans/2026-11-25-polyclave-phase1.md` Task 2（第 197 行）、Task 3（第 243 行）、Task 5（第 363 行）、Task 6（第 413–415 行）、Task 10（第 577 行）、Task 11（第 595–602 行）。
- **问题**（经本机 `codex-cli 0.172.0` 二进制与 `git` 实测确认）：
  1. **Seatbelt 路径校验**：Codex 0.172.0 的 `sandboxing/src/seatbelt.rs` 强制校验两项：所有可写根必须为绝对路径（`root path must be absolute`），且**路径中任一分量均不得为符号链接**（`writable root <path> contains symlink component <comp>; symlinked writable roots are not supported.`）。
     - 在普通仓库上执行 `git rev-parse --git-common-dir` 默认返回相对路径 `".git"`；
     - 在 macOS 上，`os.tmpdir()`（`/var/folders/...`）、`mktemp -d` 以及 `/tmp` 全部包含系统符号链接（`/var -> /private/var`、`/tmp -> /private/tmp`）。此前探针脚本在 `~/Developer/...` 下运行避开了 `/var`，而正式计划的 `tempDir()`（Task 3）、`--state-dir "$(mktemp -d)"`（Task 10）和 `tests/real-codex.spec.ts`（Task 11）全部落在临时目录下。
  2. **Git trailer 语法要求前导空行**：Task 4 第 298 行使用 `git log --format=%H%x00%(trailers:key=Run-Id,valueonly,separator=%x2C)` 解析 trailer。实测 Git 仅当提交信息最后一个段落前有**空行**时才将其识别为 trailer；若写成两行无空行（`subject\nRun-Id: r12345678`）或单行（`Run-Id: r12345678`），输出均为空字符串。而 Task 6 第 414 行 `runBrief` 只写 `Every commit message must end with the trailer line "Run-Id: <runId>".`，未提示空行或 `--trailer`。
- **具体场景**：实现者在 Task 1–10 用 `fake-codex` 全部通过（因为 `fake-codex` 不跑 macOS Seatbelt），一到 Task 11 Step 4 运行真实 Codex 验收测试，Codex 立即因 `writable_roots` / `cwd` 含 `/var` 符号链接报错退出；即便修了路径，真实模型执行 `git commit -m $'Add hello.txt\nRun-Id: ...'` 漏掉空行也会被 `submit_result` 连续拒收导致验收失败。
- **修改建议**：
  1. 在 `gitCommonDir`（用 `git -C <cwd> rev-parse --path-format=absolute --git-common-dir` 再经 `fs.realpathSync`）、`Store.createSession`、`main.ts`（`--state-dir` 创建目录后 `realpathSync`）、`prepareCodexHome` 及 `tests/helpers.ts` 的 `tempDir()` 中统一做 `fs.realpathSync` 规范化。
  2. 将 Task 6 `runBrief` 第 414 行改为：`Every commit message must have a summary line, a blank line, and end with the git trailer "Run-Id: <runId>" (for example: git commit -m "<summary>" --trailer "Run-Id: <runId>").`（`fake-codex` 的 `{ commit }` 同步按带空行的双段提交构造）。

---

### 重要（Major）

#### [M1] 优雅关机 `runner.shutdown()` 期间结束的任务轮次仍会触发纠正轮或抛未处理异常
- **位置**：`docs/superpowers/plans/2026-11-25-polyclave-phase1.md` Task 5（第 351、353、357、362 行）、Task 6（第 422–431 行）；对照 `2026-11-25-polyclave-phase1-gate-a-revisions.md` 第 14、70、86 行。
- **问题**：GPT 修订稿明确要求“关机期间不派发纠正或队列消息，关机记 `interrupted`”。但计划 Task 5 未说明 `setDispatch(false)` 后调用 `runTurn` 的行为，Task 6 结算表（第 428 行）在 `retryable` + `exit 0` 时也未检查关机状态，直接递增 `corrections` 并调 `runTurn` 发送纠正提示。
- **具体场景**：任务首轮运行期间宿主收到 `SIGTERM` 进入 `runner.shutdown()`。首轮正常退出（退出码 0）但尚未推送：若 `runTurn` 不拦截 `dispatch === false`，宿主会在关机等待中继续启动新的 Codex 纠正进程；若 `runTurn` 抛错，`Runs.start` 既白扣了一次 `corrections`，又因未捕获该错误而未能将任务干净落为 `interrupted`。
- **修改建议**：`TurnRunner` 提供 `isDispatchOpen(): boolean` 并在 `runTurn` 入口检查（关闭时抛 `DispatchClosedError`）；Task 6 结算表在 `retryable` 分支若发现 `!runner.isDispatchOpen()`，不递增 `corrections`，直接将任务更新为 `interrupted`（`detail: 'host stopped before completion; safe to retry'`）。

---

#### [M2] macOS 上 `sysctl`、`ps`、`pgrep` 与 `acquireHostLock` 的 5 处系统行为陷阱
- **位置**：`docs/superpowers/plans/2026-11-25-polyclave-phase1.md` Task 3（第 231–236、241 行）、Task 7（第 478–479 行）。
- **问题**（均在本机 macOS 实测复现）：
  1. **`sysctl -n kern.boottime` 尾部日期随时区变化**：实测同一开机周期内，`TZ=UTC` 输出 `{ sec = 1792381434, usec = 692779 } Mon Oct 19 03:43:54 2026`，`TZ=Asia/Tokyo` 输出 `... 12:43:54 2026`。若直接保存整行字符串，笔记本跨时区或夏令时切换会让 `bootTime` 字符串不等，误判为“机器已重启”从而将活跃后台写入者标为 `excluded`！
  2. **`ps -o lstart= -p` 依赖 `LC_ALL`/`TZ` 且末尾带空格**：中文 locale 与 `C` locale 输出完全不同（`"四 11月/26 ..."` vs `"Wed Nov 25 ..."`），终端与 `launchd` 环境差异会导致同一存活进程被误判为“PID 已复用（`gone`）”，甚至让第二个宿主抢走活锁。
  3. **macOS `PID_MAX = 99999`**：当测试传入 `999999` 等假 PID 时，`ps` 返回退出码 `1` 且向 stderr 打印 `ps: process id too large: 999999`；若把“stderr 非空”当成 `ps` 故障抛错，会把不存在的假 PID 判为 `unverifiable` 而非 `gone`。
  4. **`pgrep -g` 空组退出码为 `1` 及组长退出瞬间的竞态**：`pgrep` 无匹配时退出码为 `1`（退出码 `>= 2` 才是执行错误）；此外若先执行 `pgrep -g` 看到组长还在，几毫秒后组长退出，`processStartTime(pgid)` 返回 `null`，按第 241 行顺序会误判成 `leader dead but members remain → unverifiable`（实际整组已退干净）。
  5. **`acquireHostLock` 读写锁文件非原子**：未要求 `fs.openSync(lockPath, 'wx')`，两个宿主同时启动会同时读到无锁并互相覆盖 `host.pid`。
- **修改建议**：
  1. `bootTime()` 只提取 `kern.boottime` 中的 `{ sec = <n>, usec = <m> }`（存为 `"<sec>.<usec>"`），消除时区影响。
  2. `processStartTime(pid)` 固定传入 `env: { LC_ALL: 'C', TZ: 'UTC' }` 并 `.trim()`；退出码为 `1` 且 `stdout.trim() === ''` 时，若 `process.kill(pid, 0)` 报 `ESRCH`（无论 `pid` 是否超过 `99999`），统一返回 `null`。
  3. `groupLiveness` 明确 `pgrep -g` 退出码 `1` 为无成员；当 `processStartTime(pgid)` 返回 `null` 时，**再次执行 `pgrep -g <pgid>`**，二次确认仍有成员才返回 `unverifiable`，否则返回 `gone`。
  4. `acquireHostLock` 使用 `openSync(lockPath, 'wx')` 排他创建锁文件；若已存在则读取并校验持有者存活与启动时间，确认已死后再原子替换。

---

#### [M3] 解冻后未排空等待消息、HTTP 创建/重试同步阻塞语义不清、错误码映射与 SSE 事件存在缺漏
- **位置**：`docs/superpowers/plans/2026-11-25-polyclave-phase1.md` Task 4（第 314 行）、Task 5（第 346、367 行）、Task 6（第 426、435 行）、Task 9（第 538–562 行）。
- **问题**：
  1. **解冻后排队消息滞留**：会话冻结期间发入的用户消息以 `queued = true` 存储（第 367 行），但 `POST /api/sessions/:id/unfreeze`（第 540 行）未调用 `runner.drainQueues()`，解冻后消息永远停在 `waiting`。
  2. **`POST /api/runs` 与 `POST /api/runs/:id/retry` 响应时机未定义**：`Runs.start` 与 `Runs.retry` 会一直等到所有轮次结束才返回。若 HTTP 接口同步 `await` 全程，请求会挂起数分钟（Task 11 的 e2e 测试也无从在拿到 `201` 后去杀宿主或调取消）；若后台运行，则前置错误何时同步返回 `400`/`409` 必须定死。
  3. **状态码文案写反与未捕获异常变 `500`**：
     - 第 546 行 `` `409` unless `interrupted` or session frozen `` 英文逻辑写反（本意是“非 `interrupted`、会话忙或会话冻结均返回 `409`”）；
     - `Runs.resolve` 中无远端分支却调 `accept`、或有远端分支却调 `allow_retry` 未指定异常类型，在 `http.ts` 中会落成 `500`；
     - `mcp.ts` 中请求体 `JSON.parse` 或 `await check(...)` 抛错若无 `try/catch`（尤其取消与失败的 Git 检查并发时），会触发 `unhandledRejection` 直接弄崩宿主进程；
     - 会话在取消任务被冻结（第 426 行且未规定取消冻结原因文案）或被 `/unfreeze` 解冻时，未规定向 `bus` 推送 `{ type: 'session', session }` 事件；`GET /api/sessions` 的 `busy` 字段也未明确包含 `activeRun(id) !== undefined`（两轮纠正之间无 running turn 但会话仍被 run 占用）。
- **修改建议**：
  1. `/unfreeze` 写入 `host` 消息后向 `bus` 发送 `message` 和 `session` 事件，并调用 `runner.drainQueues()`；取消 run 冻结会话时固定 `frozenReason = 'cancelled while an agent was running (turn <id>)'` 并发送 `session` 事件；`busy` 明确定义为 `Boolean(runningTurn(id) || activeRun(id))`。
  2. `POST /api/runs` 在 `await runs.create(...)` 后立即返回 `201`/`200`，若 `created && run.status === 'pending'` 则后台调用 `void runs.start(run.id).catch(...)`；`POST /api/runs/:id/retry` 同步完成前置检查、远端重检及首轮认领（或直接结算）后返回 `200`，后续轮次后台推进。
  3. 修正第 546 行措辞；将 `Runs.resolve` 的非法分支条件统一抛 `ConflictError` / `StaleStateError` 映射为 `409`；Task 9 第 558 行测试明确针对 `POST /api/runs`；`mcp.ts` 对请求解析与 `check(...)` 全程 `try/catch`，并在写拒收前重验 `turnByToken` 以免已取消轮次返回 `200` 而非 `401`。

---

#### [M4] 排队中的聊天消息在 `prepared` 阶段（agent 未放行前）遇崩溃会静默丢失
- **位置**：`docs/superpowers/plans/2026-11-25-polyclave-phase1.md` Task 1（第 147 行）、Task 5（第 367 行）、Task 7（第 477、481 行）、Task 10（第 573 行）。
- **问题**：`claimTurn({ messageId })` 在创建 `phase = 'prepared'` 轮次的同一事务中把消息的 `queued` 清为 `false` 并绑定 `turnId`（第 147 行）。Task 7 第 481 行规定恢复时“绑定到中断轮次的消息保持原样”。
- **具体场景**：Turn 1 刚结束，`drainQueues()` 认领排队消息 `m2` 建立 `prepared` 状态的 Turn 2；几毫秒后（尚未执行 `authorizeTurn` 和 `go()`，Codex 进程根本没启动）宿主被强杀或断电。重启后 `recover()` 明确判定 Turn 2 为 `endReason = 'never_started'`（已严格证明 Codex 从未收到 `m2`），但 `m2` 的 `queued` 已被清掉，界面将其标为“已发给 Turn 2”且无重发入口，后续用户再发消息时 Codex 线程历史中完全缺失 `m2`。
- **修改建议**：当轮次以 `endReason = 'never_started'` 结束时（无论是 `runTurn` 启动中断还是 `recover()` 第 477 行回收 `prepared` 轮次），在同一事务中将绑定到该 `turnId` 的用户消息重置为 `queued = true, turn_id = null`；仅对已 `authorized` 的中断轮次保留绑定以防重复投递。

---

#### [M5] `buildCodexArgs` 缺少 `--` 参数终止符，以 `-` 开头的提示词会被 Codex 当成 CLI 选项报错
- **位置**：`docs/superpowers/plans/2026-11-25-polyclave-phase1.md` Task 2（第 202–212 行）。
- **问题**：`buildCodexArgs` 直接将 `prompt`（及 `resume` 下的 `agentSessionId`、`prompt`）拼在选项列表末尾，未使用 `--` 隔离位置参数。
- **具体场景**：用户在网页输入框发送 Markdown 列表（如 `- 修一下登录样式`）或提及参数（如 `--json 是干什么的`），`codex exec` 将该消息当作未知命令行参数解析并以退出码 2 失败，轮次直接变为 `failed`。
- **修改建议**：在位置参数前固定加入 `'--'`（新建：`['exec', ...flags, '--', prompt]`；续接：`['exec', 'resume', ...flags, '--', agentSessionId, prompt]`），补充一条以 `-` 开头提示词的单测。

---

#### [M6] 公开仓库的历史与早期文档含不必公开的细节（评估项 7）
- **问题**：早期提交里留有后来删掉的个人环境描述；从私有原型迁入的早期设计稿含旧原型的内部版本号、模块路径和协作细节。仓库内未检出密钥、Token 或邮箱。
- **修改建议**：清理早期文档后，把文档提交压缩为一个干净的根提交并强推 `main`。
- **处理结果（2026-11-25）**：已采纳。早期设计稿和第一次 Gemini 评审不再公开，公开历史已压缩为一个提交。

---

### 次要（Minor）

#### [L1] Task 1“双连接竞态”测试实为单线程顺序调用，且 Task 3 `fake-codex` 轮次计数缺并发互斥
- **位置**：`docs/superpowers/plans/2026-11-25-polyclave-phase1.md` Task 1（第 162 行）、Task 3（第 245 行）、Task 6（第 441 行）。
- **问题与场景**：
  1. `node:sqlite` 的 `DatabaseSync` 完全同步阻塞；在单线程测试函数里开两个 `Store` 实例先后调 `acceptRun` 根本没有并发交错，证明不了测试名 `lets two connections race on one request id` 所声称的锁竞争语义。
  2. Task 6 第 441 行 `two sessions on one repo work in separate worktrees` 会让两个会话并发启动 `fake-codex`；若 `fake-codex` 靠普通文件读写递增 `turn` 编号，两个子进程会拿到同一个 `turn` 序号跑错脚本。
- **修改建议**：Task 1 该测试改用两个 `worker_threads` 并发撞击同一 SQLite 文件（或改名反映跨连接幂等）；`fake-codex` 通过 SQLite 小表或 `mkdirSync` 锁目录原子领取 `turn` 序号。

---

#### [L2] Task 1 `.gitignore` 列表漏掉现有 `.DS_Store`，且测试裸库未一并开启 `receive.denyNonFastForwards=true`
- **位置**：`docs/superpowers/plans/2026-11-25-polyclave-phase1.md` Task 1（第 152 行）、Task 3（第 243 行）、Task 4（第 294 行）；`.gitignore`（第 4 行）。
- **问题与场景**：仓库现有 `.gitignore` 已含 `.DS_Store`，但 Task 1 Step 1 第 152 行只列了 `node_modules/, *.sqlite, *.sqlite-*, .state-*/`，实现者照单重写 `.gitignore` 会丢失 `.DS_Store`。另外 `git init --bare` 默认允许非快进强推（`receive.denyNonFastForwards=false`），只开 `denyDeletes=true` 防不住脱离组的旧进程用 `git push -f` 回退或改写已推送分支上的历史证据。
- **修改建议**：Task 1 Step 1 保留 `.DS_Store`；`makeRepo` 与 `preflightTarget`（R3）同时检查 `receive.denyNonFastForwards=true`。

---

#### [L3] 范围可裁剪项（评估项 6）：`tests/class-docs.spec.ts` 与二阶段信号写库逻辑 `TurnRunner.kill()` 可裁掉
- **位置**：`docs/superpowers/plans/2026-11-25-polyclave-phase1.md` Global Constraints（第 18 行）、Task 1（第 93、172 行）、Task 5（第 358 行）、Task 9（第 551 行）。
- **问题与场景**：
  1. `tests/class-docs.spec.ts` 用自定义文本扫描强制每个 `class` 前紧邻 JSDoc，对崩溃安全与业务逻辑零贡献，反而容易在字符串字面量或修饰符上产生误报。
  2. 第二次 `SIGINT`/`SIGTERM` 时由宿主执行 `TurnRunner.kill()` 去改写数据库轮次终态既冗余又危险（见 **[B2]**）：宿主进程一旦退出，每个子进程的 `supervisor.ts` 本就会在 2.5 秒内自动终结进程组，下次启动时 `recover()` 会走唯一经过严格验证的恢复路径完成清点与冻结。
- **修改建议**：删掉 `tests/class-docs.spec.ts`；第二次收到退出信号时直接向已知进程组发一次非阻塞 `SIGTERM` 后 `process.exit(130)`，不另起炉灶写数据库状态。

---

## 三、各评估维度结论速览（评估项 2–7）

1. **Revision 2 新引入的矛盾与接口错位（评估项 2）**：主要集中在 `correctionText` 与 `fake-codex` 合同脱节、`/unfreeze` 写入的 `host` 消息屏蔽聊天中断提示、`Runs.create` 重放短路与 `ConflictError` 冲突、`Store` 缺失 4 个下游必需查询方法（**[B5]**），以及 `classifyRun` 漏标 `observedSha`（**[B3]**）和 HTTP 同步/异步与状态码定义偏差（**[M3]**）。
2. **崩溃安全性（评估项 3）**：
   - *未完成被标为 `done`*：`Runs.resolve('accept')` 不核对 `observedSha` 与基线（**[B3]**）；`Store.endTurn` 无条件覆盖可能抹掉取消/强退留下的不确定状态（**[B2]**）。
   - *重复运行*：并发 `Runs.retry` 穿过异步检查窗口重入 `claimTurn`（**[B1]**）；聊天取消或 `runner.kill()` 后未冻结即拉起新轮次与残留工具并发写（**[B2]**）。
   - *工作或消息丢失*：`acceptRun` 后、`prepareWorktree` 前崩溃导致后续 `retry` 目录缺失失败（**[B4]**）；`prepared` 轮次崩溃导致已出队的聊天消息静默丢失（**[M4]**）。
   - *冻结会话启动轮次*：数据库 `claimTurn` 已对已冻结会话做强拦截，但聊天取消和 `runner.kill()` 存在“理应冻结却未冻结”的漏口（**[B2]**），且 `sysctl kern.boottime` 随时区变化可能误解除冻结保护（**[M2]**）。
3. **工单可执行性（评估项 4）**：Task 3 存在确定性 `tsc` 类型错误与监护进程定时器竞态（**[B6]**）；Task 4 存在 `observedSha` 缺失与聊天轮次调 MCP 拒收写库异常（**[B3]、[B5]**）；Task 6 漏调 `prepareWorktree`、漏处理优雅关机且 `correctionText` 破坏测试夹具（**[B4]、[B5]、[M1]**）；Task 8 第 1、5 行崩溃实验在“禁用 `sleep`”约束下缺少宿主侧屏障与并发通道（**[B7]**）。
4. **macOS + Node 22.19+/24+ + Codex 0.172.0 可行性（评估项 5）**：
   - **已实测通过的部分**：技术栈锁定的包版本在 npm 均真实可用；Node 原生类型擦除可直接运行 `.ts`（注意本地相对导入必须写 `.ts` 后缀，因 `noEmit` 下 Node 运行时不会把 `.js` 重写回 `.ts`）；`node:sqlite` 唯一约束冲突实测确为 `code = 'ERR_SQLITE_ERROR'`、`errcode = 2067`（注意：手写 `BEGIN IMMEDIATE` 事务回滚须用 `try { db.exec('ROLLBACK') } catch {}` 防止 SQLite 自动回滚后二次报错掩盖 `2067`，且 Node 22 下 `node:sqlite` 会向 `stderr` 打印 `ExperimentalWarning`，测试解析子进程输出时只应读 `stdout`）；macOS 上 `stdio` 第 4 管道实测为双向 UNIX socketpair，可正常收发 `ready\n` / `go\n` 并在宿主 `SIGKILL` 时立即触发 EOF；`codex exec` 与 `codex exec resume` 均支持 `-c` 注入全部沙箱与 MCP 参数；裸库按原始 40 位 SHA `git fetch`（含非分支顶点的历史提交）实测可行。
   - **实测暴露的平台阻塞与隐患**：Codex 0.172.0 Seatbelt 报错拦截含 `/var`、`/tmp` 符号链接分量的路径及相对 `.git`、Git trailer 要求前导空行（**[B8]**）；`ChildProcessWithoutNullStreams` 类型不兼容与 fd 3 / 管道生命周期（**[B6]**）；BSD `ps`、`pgrep`、`sysctl` 的时区、`PID_MAX = 99999` 与退出码细节（**[M2]**）；提示词前需加 `--`（**[M5]**）。
5. **范围裁剪（评估项 6）**：见 **[L3]**（可移除 `tests/class-docs.spec.ts` 与 `TurnRunner.kill()` 的独立写库逻辑）。
6. **公开仓库内容审查（评估项 7）**：见 **[M6]**（已处理：早期文档不再公开，历史已压缩）。

---

## 四、最终结论（Verdict）

**阶段一实现目前尚不可按原样启动。**

虽然 Revision 2 的核心架构（实例级所有权、监护进程放行握手、单裸库目标、独立 worktree、统一 `classifyRun` 裁决表与会话冻结模型）方向扎实，但工单中仍存在 **8 项阻塞问题（`[B1]`–`[B8]`）**：既有会导致未审提交被标为完成、并发重试双跑、脱离组写入者漏冻结的崩溃安全漏洞（`[B1]`–`[B4]`），也有按工单逐字实现必然触发 `tsc` 编译报错、测试契约自相矛盾、缺少测试屏障以及在 macOS 上跑真实 Codex 0.172.0 必败的硬缺陷（`[B5]`–`[B8]`）。

**开工前必须先在计划（及对应的 v4 设计条款）中修完的清单**：
1. **必改阻塞项**：**`[B1]`、`[B2]`、`[B3]`、`[B4]`、`[B5]`、`[B6]`、`[B7]`、`[B8]`**（共 8 项，全部只需在接口签名、事务前置条件、提示词模板和夹具协议上做局部修正，无需推翻任何已定架构决策）。
2. **强烈建议同一轮顺手合入的重要项**：**`[M1]`**（优雅关机结算分支）、**`[M2]`**（macOS `sysctl`/`ps`/`pgrep` 与原子文件锁细节）、**`[M3]`**（解冻排空队列与 HTTP 同步/异步及 `409` 映射）、**`[M4]`**（`never_started` 轮次归还排队消息）、**`[M5]`**（`buildCodexArgs` 增加 `'--'`），以及在拉出 `feat/phase1` 分支前按 **`[M6]`** 决定是否压缩重写 `main` 的初始文档提交历史。
