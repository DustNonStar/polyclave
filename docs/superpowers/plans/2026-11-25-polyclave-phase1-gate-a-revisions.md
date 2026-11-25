# Polyclave 阶段一修订稿：崩溃恢复与 Gate A

**给计划作者 · 待审 · 基于 `0cb4c84`。** 请先把下列条款和断言合入 [v4 设计](../specs/2026-11-25-agent-console-design-v4.md) 和 [阶段一工单](2026-11-25-polyclave-phase1.md)，再交实现者从 Task 1 开工。本文中的测试都是拟议验收条件。与现行合同冲突的测试列在对应 Task，由计划作者一并修订。阶段一仍使用 Codex、本地网页、SQLite 和本地裸库，Task 8 后停工评审。

需要守住的恢复顺序是：**先确认旧执行能否继续写，再核对固定远端，最后决定任务状态和会话能否继续调度。** 单看数据库中的退出码、一个空的进程组或一个不存在的分支，都可能放行第二个写入者。

| 上轮阻塞项 | 主要落点 | Gate A 可观察的结果 |
|---|---|---|
| 1 · 工具进程脱离外层组 | Task 3、5、7 | 外层组已空但旧工具仍可写时，会话冻结，手动重试和队列均无法启动新轮次 |
| 2 · PID 复用及先启动后登记 | Task 1、3、7 | 同 PID 的新宿主能回收旧轮次，未登记的 supervisor 无法启动 agent，身份不明的组无人误杀 |
| 3 · 推送和核对指向不同裸库 | Task 2、4、6 | fetch/push 目标分离时拒绝建任务，建好后改 origin 不会改写投递目标 |
| 4 · 共用 checkout、基线漂移 | Task 1、4、6 | 两个会话改同一文件互不串写，错误基线即使 trailer 齐全也无法结算 |
| 5 · 已受理任务搁置或重复启动 | Task 1、5、6、7 | 请求重放得到同一 run，`pending` 重启后有去向，每条排队消息只绑定一个 turn |
| 6 · 退出、取消、恢复的裁决分叉 | Task 4、6、7 | 转人工、成功及可重试都有同一组证据；关机期间不派发纠正或队列消息 |
| 7 · MCP 查令牌与写收据之间存在竞态 | Task 1、4 | 取消先提交则迟到请求得到 401；并发相同报告只生成一条 accepted |
| 8 · fake、提示词与 harness 合同冲突 | Task 3、5、8 | 强杀前的 prompt 可查，回执使用实际 SHA，重启后有可访问的 MCP 端点，推送次数可独立计数 |

## Task 1 · 把受理、占用和执行身份写进 Store

**设计改动。** 在 v4 §7.4–7.5、§8.2 规定三种不同的身份：一次用户请求用 `requestId`，一次逻辑任务用 `runId`，每个持锁宿主启动时生成新的 `hostInstanceId`。保留 PID、启动时间用于进程诊断。R2 的占用期从请求受理开始，覆盖准备 worktree、首轮以及两次纠正之间的空档。另设持久的会话冻结原因，以便 run 转人工后仍能禁止派发。

**工单改动。** 在 Task 1 的 row、索引和 Store 接口中写明下列事务，Task 5–7 直接复用。

1. 受理时一笔事务写入请求键、规范化输入、`pending` run、session 占用，以及固定目标 `remote/baseSha/branch/worktreePath`。同一 session 的同一键和同一输入返回已有 run，包括它已经完成的情况。同键不同输入返回冲突。另一请求遇到已有占用时也返回冲突。建 worktree 和启动子进程都在提交之后。
2. 启动时用带前置状态的更新认领 run 并建立 turn，保存 `hostInstanceId`、本轮 token 和启动阶段。保留“每个 session 至多一个 running turn”的唯一索引，再用数据库约束保证同一 session 至多一个占用中的 run。重试次数随成功认领一起递增。
3. 投递排队消息时，在同一事务认领最早消息、建立 turn、把原消息 ID 绑定到 turn。崩溃后能显示“已领取但执行中断”；恢复不能因缺少回复而把同一条消息悄悄投递两次。
4. 保存 turn 的结束原因、停止意图和写入者核对结果，并保存其依据，例如“未放行”，或当时的系统启动标识以便与当前启动标识比较。无法取得证据时显式记录未知。每个 run 至多有一条 accepted 收据，拒收仍可多条。关闭 turn 或开始取消时使其 token 失效。冻结及未解除的原因必须能跨第二次宿主启动保留。

**Gate A 断言。** 两个数据库连接同时认领同一请求，只得一个 run 和一个会话占用；同键改 instructions 得冲突。受理事务提交前强杀，重送可建立一条；提交后、首轮前强杀，重送返回原 ID。消息领取事务前后分别中断，断言同一消息至多绑定一个 turn，原始消息始终可查。

## Task 2 · 固定每轮的工作目录和 Git 目标

**设计改动。** 在 v4 §6.8、§7.2 明确普通聊天使用会话目录，任务首轮、纠正和重试使用该 run 的 worktree。任务使用的 Codex thread ID 应与其 worktree 一起保存，避免把主聊天或另一任务的 thread 续接到新的目录。

**工单改动。** 修改 Global Constraints 的启动参数与 Task 2/5 的接口。显式传递每轮的 cwd、Codex thread ID 和 writable roots。run 使用其固定 worktree、绝对 Git common dir 和固定裸库，保持 `networkAccess=false`。Task 2 测参数构造；裸库预检和 worktree 创建分别归 Task 4、6。把 Task 6 brief 和 Task 3 fake 中的 `git push origin <branch>` 都换成向已固定的裸库绝对路径推送，使用完整 refspec `refs/heads/<branch>:refs/heads/<branch>`。路径传给进程时保留参数边界，展示在提示词中时正确引用。

**Gate A 断言。** 路径含空格和引号时，fake 的实际 cwd、写入根和 Git 收到的目标都与持久化值相同。聊天 → 任务 → 回到聊天，检查三次启动的 cwd 和 thread ID；同一任务 retry 复用自己的 worktree 和已记录的 thread。

## Task 3 · supervisor 先登记再放行，单独判定剩余写入者

**设计改动。** 改写 v4 §6.8 第 3 条、§7.5 和 §19 的进程清理承诺。Codex 0.172.0 的管道执行在 Unix 可创建新 session，macOS 后端支持 `POSIX_SPAWN_SETSID`。外层 PGID 为空只能证明该组已空。原探针 §9 验证了特定任务的远端结果，未覆盖这种工具进程。 [Codex 管道实现](https://github.com/openai/codex/blob/rust-v0.172.0/codex-rs/utils/pty/src/pipe.rs#L149-L167) · [macOS 启动实现](https://github.com/openai/codex/blob/rust-v0.172.0/codex-rs/utils/pty/src/posix_child.rs#L208-L218) · [探针 §9](../research/2026-11-24-t3-probe-results.md#9-崩溃恢复v4-第一版验收目标2026-11-25)

**工单改动。**

1. 保留单宿主互斥。取得锁后创建本次实例 ID。supervisor 启动后经私有 IPC 报告 ready，并等待一次性放行。宿主先提交 turn、supervisor 身份、PGID、启动时间和“允许执行”，再发放行消息。未收到放行就遇到 EOF，supervisor 直接退出。数据库尚未授权时可以证明 agent 未启动；已授权而确认丢失时按“可能已启动”处理。
2. 分开记录“外层组状态”与“本轮是否仍可能写 worktree 或远端”。组长和记录匹配才可按组清理；组长消失时，只有额外身份依据能把成员归到该轮，才能针对已验证对象清理。查询失败、启动时间缺失、成员归属不明时保留 `unverifiable`。发送信号后重新观察；supervisor 转发 TERM 要防止重复处理自身信号，也要让 KILL 升级在直接子进程先退出时仍有效。
3. **阶段一建议采用保守退路。** 同一系统启动周期内，已放行的真实 CLI 被硬杀，且无法证明所有工具写入者均已退出时，持久冻结该会话并转人工。允许放行的证据必须在工单写明，例如未曾授权启动，或可验证的系统重启已清除旧进程。单次进程快照、等待三秒或过期 MCP token 都无法排除迟到的 Git 写入。如果作者选择通过撤销写权限放行，验收须同时覆盖 worktree 和裸库两处写入。
4. 修改 fake：解析完参数立即落一条 `prompt_started`，正常退出才另记结束事件。拆出“只提交”和“推已有提交”动作；增加“报告当前已推结果”动作，由本轮上下文取真实 run ID、branch 和实际推送 SHA；保留传入原始错误对象的动作。增加可等待、可释放的检查点，以及自建进程组并等待写入的工具夹具。

**Gate A 断言。** 在 supervisor ready、身份提交、放行这三个边界分别杀宿主。未放行的 agent 启动记录为零；已放行且身份不明时，重复恢复也不会启动第二轮。单独覆盖组长被替换、组长消失但仍有身份不明的成员，断言无关进程仍活着。保留原 3 秒同组清理测试，先等孙进程 ready 再触发终止，按其实际 PID 验证退出；跨组用例另见 Task 8。

## Task 4 · 核对固定远端，并原子接收 MCP 回执

**设计改动。** 在 v4 §6.4、§7.5、§8.2 和 R1/R3 中定义宿主接受的证据。任务的基线是受理时的 commit SHA。一个可交付的 tip 必须从该基线派生，至少多一个提交，区间内每个提交的 Git trailer 都带该 run 的正确标识。

**Git 工单。** 受理前解析 `origin` 的 fetch URL 及 `git remote get-url --push --all origin`。阶段一只接受单一 fetch 和单一 push 目标，两者规范化后指向同一个本地裸库。拒绝 URL、多个推送目标和任何把实际投递改向别处的重写。由宿主在裸库解析固定 `baseSha`。检查时可直接在该裸库取任务 ref 的 tip 一次，后续祖先、区间及 trailer 检查全部使用取得的 OID；查询失败与 ref 确认不存在分别编码。使用 Git trailer 解析规则，提交正文中引用的一行 `Run-Id` 无法代替 trailer。 [Git remote](https://git-scm.com/docs/git-remote#Documentation/git-remote.txt-get-url) · [Git trailers](https://git-scm.com/docs/git-interpret-trailers)

**给 R3 补充分支保留前提。** 本稿建议阶段一使用受控的本地裸库，任务分支在结算前只能通过 Git 接收路径更新，预检要求该路径拒绝删除；例如在专用测试裸库启用 `receive.denyDeletes=true`。宿主不擅自修改用户裸库。未满足此前提的仓库拒绝建任务。若还需支持任务期间人工直接删 ref、替换裸库或回滚备份，就要增设独立持久的远端推送收据；这些操作可让“从未推送”和“推送后删除”留下相同现场。证据链失效时转人工，不能由一次空查询得出可重试。

**MCP 工单。** 请求进入时校验 token 和 schema，在 SQLite 写事务外做 Git 检查。结束时用短事务重新验证当前宿主实例、该 turn 正在接受结果、run ID 与会话占用一致，并写 accepted。取消、结束、关机撤销令牌使用同一组事务条件。有效令牌重复提交相同 branch/SHA 返回 `Already accepted.`；不同 SHA 不得替换第一条 accepted。回执提交后再答成功；最终 run 状态由 Task 6 或 7 用当前远端重新结算。

**Gate A 断言。** 两个同样可写的裸库 A、B 分别作为 fetch、push 目标，建任务失败且两边均无新 ref。正常建任务后改 origin，实际推送与检查仍只发生在 A。错误祖先、伪 trailer、空提交区间分别被拒。保留策略拒绝删除已推的未报分支，恢复保留原 SHA。用屏障暂停 MCP 的 Git 检查，此时取消并提交撤销事务，再释放检查，断言 HTTP 401、accepted 行数为零；并发两个相同有效请求得到一次接受及一次幂等响应，accepted 行数为一。已有收据时提交不同 SHA，第一条收据保持可追溯。

## Task 5 · 统一调度入口和中断提示

**设计改动。** 在 v4 §7.2、§7.5 和 R2 明确冻结作用于整个 session。新用户消息仍可入库并显示等待；普通发送、队列排空、创建任务、纠正、手动重试都要经过同一占用与冻结检查。关机一开始先关闭新派发，再撤销 token、等待已在途的处理并清理进程。

**工单改动。** `runTurn` 只经 Task 3 握手启动，使用 Task 1 已认领的 turn。runner 记录退出事实，run 的结算交给 Task 6。在同一事务或可重放的持久步骤内把中断提示绑定到本次发送。普通聊天仅在上一轮确实中断时添加一次 `Note from the host`；run 的手动 retry 由 `runBrief(run, true)` 独占提示前缀 `This task was interrupted earlier`。Task 11 的聊天验收同步分成“首轮完成后重启”和“已收到 thread ID、首轮尚未完成时重启”。

**Gate A 断言。** 已完成聊天重启后沿用 thread ID，下一条无中断提示；运行中聊天重启并确认可续接后，首条有提示，第二条没有。retry 的实际 prompt 以约定的 run 前缀开头且只含一条中断说明。冻结时发送一条消息、调用 drain、再重启，消息仍为同一 ID 且 queued，新增 prompt 为零。关机期间收到迟到的 turn 结束事件，也不会启动纠正或下一条消息。

## Task 6 · 宿主准备 worktree，所有出口共用结算表

**设计改动。** 把 v4 §9.2、§10.4 已规定的独立 worktree 落到阶段一。v4 §6.4 的两次纠正只适用于仍可安全继续的任务。§7.5 和 R1 区分 turn 退出原因、run 的交付证据，以及下一步是否允许调度。

**工单改动。** `Runs.create` 接受 `requestId`。同键重放先返回已有记录，再考虑新请求所需的预检和 busy 判断。首次受理保存 Task 4 的目标，宿主在落库后从固定 `baseSha` 建 `agent/<runId>` 和独立 worktree。第一次和第 N 次准备都先核对已有分支、路径、归属和基线；保留合法的未提交改动和本地提交。遇到路径冲突或准备现场无法解释时保留现场、转人工。普通聊天目录及其当前分支不得随任务切换。

**将以下表写入 R1，Task 4 的 `classifyRun` 为唯一交付判据。** 正常退出、非零退出、取消、会话恢复和手动 retry 都依次使用相同证据。Task 6 只在得到最后一行时，才按轮次结束原因选择下一动作。

| 已知事实，按行优先 | 持久结果与允许的动作 |
|---|---|
| 旧执行仍可能写入，或所有权无法确认 | run 为 `needs_review` 并冻结 session；普通聊天中断同样冻结 session。重新读远端不能单独解除冻结 |
| 写入者已排除，远端读取失败 | `interrupted`，保存 `remote check failed: …`；retry 须重新核对成功 |
| accepted 的 SHA 等于当前 tip，祖先、非空区间和全部 trailer 合格 | `done`，保存 `resultSha` 和来源；同时保留 turn 的实际退出码及取消记录 |
| 已推无 accepted；或已有 accepted 但 ref 消失、tip 不同、区间不合格 | `needs_review`，保存原因和观测到的 SHA，保留分支和 worktree，纠正与重试均为零 |
| 确认无远端分支、无 accepted，且满足分支保留前提 | 得到 `retryable` 证据。普通成功退出且漏交可纠正，最多两轮；用尽后 `failed`。立即非零退出记 `failed`。取消、关机和崩溃记 `interrupted`，等用户重试。显式 retry 才认领新一轮，最多三次 |

每次真正启动前重新检查 session 占用、冻结和关机状态。三次限制只限制再次执行；在点击 retry 时已经满足 `done` 或 `needs_review` 的证据，应直接结算。需要第四次执行时沿用 `retry limit reached`。每次离开 running 后，队列也须经过这些条件。

**Gate A 断言。** 两个 session 指向同一源 checkout，同时在各自任务中改同名文件。断言两个 worktree 不同、各自远端 SHA 的文件内容正确，用户 checkout 的 HEAD 和原有改动保持原样。按上表测试“推送＋accepted＋非零退出”“推送后退出但未报”“取消时已推”“关机时远端为空”。原测试 `pushed but never submitted: needs_review after two corrections, branch kept` 由计划作者改成“已推未报立即转人工、无纠正、分支仍在”。保留“两次纠正后失败”的无推送用例；“一次纠正后成功”的首轮无推送，第二轮才推并交回执。

## Task 7 · 恢复覆盖每一条已受理记录

**设计改动。** v4 §7.5、§8.3、§17 统一为先恢复、后开放会话。对有外部动作的 worker，续接和重开都遵守本节顺序。§17.1 的固定内容探针继续标为历史实测，新增场景的通过情况由 Gate A 单列。

**工单改动。** 获取独占宿主锁并创建新实例 → 禁止派发与 MCP 收件 → 按实例找旧 turn、撤销旧 token 并保留启动证据 → 按 Task 3 确认旧写入者 → 遍历 `pending/running/interrupted` 及已有冻结 → 用 Task 4/6 的规则生成逐 run 的 `RecoveryReport` → 开放 MCP、runner 与队列。Task 8 harness 与未来 Task 9 `main.ts` 共用这条启动顺序。恢复过程自身可再被强杀；每一步重入均不得新建 run、清空冻结、重复推送或自动重试。

已受理、尚未放行的 `pending` 也核对远端和 worktree 准备现场。现场一致且远端无痕迹时，同一 run 变 `interrupted`、报告 `retryable`，原请求键仍指向它，只允许显式 retry。已放行的旧 turn 即使 `hostPid` 恰好等于当前 PID，仍按旧实例处理。冻结时仍可只读记录当前远端 SHA；这是一条带观测时间的证据，不能据此宣告旧写入者已经退出。把“保留当前宿主轮次”测试的判据改为相同实例，另加相同 PID、不同实例的反例。

**Gate A 断言。** 在受理后、worktree 创建后、授权前后、收据提交后分别中断，连续恢复两次，核对 run ID、收据数、冻结原因、队列绑定和远端 ref。身份不明时 report 列出对应 turn/session；没有 run 的聊天也不能被 drain 偷偷续接。已证实无旧写入者、远端无分支时，只有显式 retry 增加 prompt 数和 retries。

## Task 8 · 把 Gate A 写成可判定的崩溃实验

**设计与工单改动。** 在 v4 §17/§17.1、计划的 Review Focus、Handoff and Acceptance 和 Task 8 同步以下验收。harness 增加“恢复并等待释放”模式：恢复完成后经 IPC 发送 ready、MCP URL 和报告，父测试可发请求并显式结束 harness。保留一次性 `recover` 命令。测试用 IPC 或可观察的数据库/裸库提交作屏障；超时只用来判失败。每个用例使用独立状态目录、裸库和夹具日志。

每次 fake 准备执行 push 时记一次尝试；裸库 `post-receive` 独立记录成功的 `(ref, oldSha, newSha)`。初始 main 的推送不计入任务 ref。相同 SHA 再 push 可能没有 ref 更新，两种计数都要断言。检查旧令牌时经父测试私有通道取值，公开日志和 SSE 均不得输出 token。

| 用例和精确断点 | Gate A 必须观察到 |
|---|---|
| 同一请求并发两次，再模拟响应丢失后重送；另在 `pending` 首轮前杀宿主 | 一个 run ID、一个占用；重启后该 ID 可人工重试；并发两个 retry 至多启动一个新 turn |
| fake 工具已在另一进程组 ready，杀宿主和外层组，确认外层 PGID 已空；工具在测试屏障后才尝试写 | 按 Task 3 保守方案，run 转人工、session 冻结、无新 prompt。释放旧工具使其写入测试标记，证明外层组检查确会漏掉写入者。若采用写权限撤销实现，则必须证明旧工具写 worktree、推远端两项均失败，才能放行新任务 |
| supervisor 一并被杀，留有成员；另造无关或无法验证归属的成员 | 对有独立身份依据的旧成员验实际 PID 已退出；对无身份依据的成员验其仍活着、会话冻结。修订原“无组长也必须杀整组”的断言 |
| 已推半成品、无 accepted，在第二部分开始前杀宿主 | `needs_review`，任务 ref 等于第一次推送的 SHA，恢复两次后总 push 尝试 1、ref 更新 1、首轮 prompt 1、纠正 0。原因保留已知远端事实；另有旧写入疑点时还要报告冻结 |
| 首轮授权前被杀；另测已放行且归属不明的硬杀 | 前者确认无写入后为 `interrupted/retryable`，显式 retry 后 `done`，分支上一个提交。后者冻结。将原“看到 turn.started 后杀掉即安全重试”拆成两例 |
| 在本地 worktree 准备一个带正确 trailer 的未推提交 X，记录原 SHA，模拟可证明已停写的中断现场 | 恢复无自动 push；retry 在同一 worktree 复用 X，远端 tip 和回执都是 X，区间一个提交、一次 push 尝试。注明这是预置现场，与上一行的真实进程强杀分别验收 |
| 已提交 accepted(X)、已确认写入结束，在写 run 终态前强杀；再测 accepted(X) 后远端变为 Y | 前者 `done`、`resultSha=X`、一条 accepted；后者 `needs_review`、保留 Y。若强杀发生在仍可能写入的时刻，按冻结用例断言 |
| 第一宿主持锁、另一 harness 同时启动；随后第一宿主崩溃，新 harness 恢复并发送 ready | 活跃期间第二个宿主未执行恢复或派发。向新端点提交旧 token 得 401，accepted 行数不变。新端点保持可访问直到测试发出释放信号 |

**审阅者签收条件。** 在干净 checkout 上运行 `pnpm check`，再连续三次运行 `pnpm test tests/milestone.spec.ts`。核对 Task 1–7 的针对性断言和上表的 SQLite 行、裸库 ref、进程 PID、prompt 与 push 计数。检查每次写 `done` 都消费统一判据、每次派发都检查冻结、每个 agent 都经握手启动；审阅 `PLAN-ISSUES.md`。若实现只有“PGID 为空”的证据，却将已放行的未知写入者判为安全重试，Gate A 不通过。

**下游同步。** Task 9 的创建请求携带客户端生成的 `requestId`，busy/冻结返回冲突，同键同输入重放返回原 run；Task 10 同一次提交重发沿用原键。Task 9 采用 Task 7 的启动顺序及 Task 5 的停机顺序。Task 11 沿用 Task 5 的提示词规则和 Task 8 的确定性断点。以上仅修正后续接口和测试合同，实际实现仍按 Gate A 的顺序交接。
