# Orca 对照笔记

- 日期：2026-11-25
- 对象：[stablyai/orca](https://github.com/stablyai/orca)，MIT 许可，读取版本为提交 `d9884b7`（2026-11-25）。只读源码和文档，没有安装运行。
- 目的：Orca 是与 T3 Code 同类的桌面 agent IDE（Electron + TypeScript，带 iOS/Android App）。看它怎么处理我们 v4 方案关心的问题，决定借鉴什么、避开什么。
- 结论先说：**值得借鉴，不适合当底座。** 它的编排由协调模型驱动，并且把 worker 自报的完成结果直接当真；Polyclave 第一版要证明的"按外部状态核对后再判定完成"，正是它没有做的。

以下路径均相对 Orca 仓库根目录。`O` 表示 `src/main/runtime/orchestration`。

## 1. 编排与崩溃恢复

| 方面 | Orca 的做法 | 出处 |
|---|---|---|
| 存储 | SQLite（WAL、`busy_timeout=5000`），51 个版本的迁移；表：runs、messages、deliveries、mutation_receipts、attempt_observation_facts、worker_dispatches、tasks、dispatch_contexts、decision_gates | `O/db/orchestration-db.ts:61-73`，`O/db/schema/` |
| 状态变更 | 统一走"比较后再写"的转换函数，事务用 `BEGIN IMMEDIATE` | `O/db/lifecycle-transition.ts` |
| 完成权 | worker 用 `orca orchestration send --type worker_done --task-id --dispatch-id --outcome` 报完成；发送方必须是该次执行绑定的终端和进程实例（`ptyId:incarnationId`），只知道 id 不算数 | `O/workers/reports/worker-report-admission.ts:33-52`，`O/workers/lifecycle/lifecycle-reconciliation.ts:17-28` |
| 结算 | 一个事务内完成：同结果重复报告按幂等处理（返回 `duplicate: true`）；对已结算的执行返回 `inactive_dispatch`；不是最新一次执行返回 `stale_dispatch`；结算后作废这次执行的凭证 | `O/db/dispatch-context/worker-report-settlement.ts:75-200` |
| 结果核对 | **不核对**。`filesModified` 等照单全收；唯一的 git 检查是派活前"基线落后超过 20 个提交就拒绝" | `lifecycle-reconciliation.ts:278-294`，`O/coordinator/dispatch/coordinator-stale-base-flag.ts` |
| worker 进程死亡 | 判该次执行失败并通知协调者；同一任务失败 3 次熔断，否则任务回到 ready | `O/workers/lifecycle/worker-exit-observation.ts`，`dispatch-circuit-breaker.ts` |
| 应用重启 | 终端放在独立的守护进程里，agent 可以活过应用重启；启动时：旧运行时期（`runtime_epoch`）里开始的执行标为 `start_unknown`；终端不见了就判失败；存活状态分三种——存活、已退出、无法确认，"无法确认"永远不触发自动重试 | `O/db/worker-dispatch/restart-cut-worker-start.ts`，`O/terminal-authority/release/worker-terminal-process-liveness.ts`，`skill-guides/orchestration/references/recovery-and-cleanup.md` |
| 并发 | 一个终端同时只有一个执行（插入前检查）；一个任务只能启动一次；没有用唯一索引，靠 `BEGIN IMMEDIATE` 下的代码检查 | `O/db/worker-dispatch/worker-terminal-claim.ts:33-47`，`O/db/tasks/task-start-claim.ts` |
| 调度者 | 主路径由协调模型通过命令行建任务、启动 worker；代码只做依赖图的记账（依赖完成后把子任务提为 ready）。另有一个旧的、只在内存里的确定性调度器 | `skill-guides/orchestration/references/coordinator-loop.md`，`O/db/tasks/task-store.ts`，`O/coordinator/coordinator.ts` |
| 权限提示 | 靠看终端屏幕、Claude 钩子或聊天记录发现，上报给协调者；只能"允许一次"或"拒绝一次"，不能留下长期规则 | `O/workers/approvals/worker-approval.ts:40-48` |

## 2. 各家 agent 的接入

| 方面 | Orca 的做法 | 出处 |
|---|---|---|
| Codex | 结构化聊天用长期运行的 `codex app-server`（JSON-RPC），多个聊天共用一个进程；`thread/start`、`thread/resume`、`turn/start`、`turn/interrupt`；**只以 `turn/completed` 作为一轮结束**；批准请求以服务端请求的形式到达；没有用 `codex exec` | `src/main/codex/structured/launch/codex-structured-launch-resolution.ts:214`，`.../journal/codex-structured-journal-translation-turn-boundaries.ts:162` |
| Codex 续接 | 线程 id 持久保存；续接时处理三种错误：已归档（先取消归档）、找不到记录（只有从未对话过才新开）、已有别的写入者（拒绝） | `src/main/codex/structured/threads/codex-structured-thread-open.ts:118-130` |
| Claude | 用 `@anthropic-ai/claude-agent-sdk` 的流式输入模式，指向用户本机的 `claude`；以 `session_state_changed` 变为 `idle` 作为一轮结束；会话 id 由固定规则生成（`sha256("orca-claude:"+sessionId)`） | `src/main/claude/claude-stream-json-connection.ts`，`.../turns/claude-session-state-turn-over.ts`，`.../lifecycle/claude-structured-session-id.ts` |
| Antigravity 等 | ACP：`agy acp`，关闭自动更新；其他 ACP agent 每家一个方言文件；续接优先 `session/resume`（`session/load` 会重放全部对话） | `src/main/antigravity/antigravity-acp-launch-spec.ts`，`src/main/acp/acp-dialects/` |
| 注入自己的工具 | Codex 在 `thread/start` 里按线程传 `mcp_servers.*`；Claude 在 SDK 启动参数里传；ACP 在建会话时传。都不写进用户的配置文件 | `src/main/bots/bot-tools-codex.ts:21-50`，`bot-tools-claude.ts`，`src/main/acp/launch/acp-mcp-server.ts` |
| 用户配置 | **复用而不是隔离**：Codex 按账号镜像并合并 `~/.codex/config.toml`；Claude 的配置目录软链接 `~/.claude` 的大部分内容；终端模式下把钩子装进用户全局的 Claude/Codex 配置 | `src/main/codex/configuration/settings/codex-config-mirror.ts`，`src/main/claude-accounts/profiles/setup/claude-profile-provisioning.ts`，`src/main/claude/hooks/hook-settings.ts:61` |
| 进程管理 | 每个 agent 外面套一个监护进程：自身是独立进程组的组长，发现宿主进程没了（`process.ppid !== ownerPid`）就结束整组；停止时先关输入，1 秒后 SIGTERM，3 秒后 SIGKILL；记录进程组以便崩溃后清理；按"进程启动时间"判断 pid 是否被复用，判断不了就当"无法确认" | `src/main/provider-process/provider-process-supervisor.ts`，`provider-ownership-journal.ts`，`provider-group-orphan-guard.ts` |

## 3. 远程、手机、推送、安全

| 方面 | Orca 的做法 | 出处 |
|---|---|---|
| 配对 | 二维码含连接地址、设备令牌、宿主公钥（手机固定这个公钥）；二维码里是一次性票据，10 分钟内换成客户端自己提出的长期凭证；撤销只在宿主端做 | `src/shared/remote-runtime/pairing/`，`src/main/runtime/rpc/server/pairing/pairing-ticket-exchange.ts` |
| 连接 | 直连（局域网或 Tailscale 地址，端口 6768，普通 `ws://`，应用层端到端加密：X25519 + HKDF + secretbox）；或经 Orca 自己运营的中转（需要 Orca 账号，中转只转发密文） | `src/shared/remote-runtime/e2ee/`，`cloud/README.md` |
| 推送 | 经 Orca 自己的推送网关 `push.onorca.dev` 转发到 APNs/FCM；**载荷带标题（80 字）和正文（180 字）**，文档写明没有"不带内容"的模式；`ORCA_MOBILE_PUSH_MODE=disabled` 可关闭 | `cloud/packages/push-contract/src/send-messages.ts:9-37`，`docs/site/content/docs/notifications.mdx` |
| 本机接口 | Unix 套接字 + 每次启动生成的令牌，令牌存在权限 600 的文件里——同一 macOS 用户的进程都能读到 | `src/main/runtime/rpc/server/runtime-rpc-request-admission.ts` |
| Bots | 跑在用户自己的电脑上，用用户自己的 Claude/Codex 登录；Slack、Linear、GitHub 的事件经 Orca 的云服务转进来 | `src/main/bots/`，`src/main/connector-cloud/` |
| 遥测 | PostHog，默认开启，匿名、不含内容；`ORCA_TELEMETRY_DISABLED=1` 或 `DO_NOT_TRACK=1` 关闭 | `src/main/telemetry/`，`docs/site/content/docs/telemetry.mdx` |

## 4. Polyclave 采纳什么

阶段 1（已写入 v4 方案 §7.5 和阶段 1 计划）：
1. **监护进程**：宿主死后 agent 进程组自动结束；启动时的残留清理保留作兜底。
2. **按进程启动时间确认身份**，取代按命令行文字匹配；确认不了就是"无法确认"。
3. **三值存活状态**：存活、已退出、无法确认；"无法确认"不自动处理，相关任务转人工核对。
4. **每一轮一个宿主工具令牌**，这一轮一结束（完成、取消、中断）就失效。
5. **重复提交按幂等处理**：同一任务交来同样的结果，回答"已接受"，不重复记录。
6. **任务重试设上限**（3 次），超过就转人工核对。

后续阶段（写入方案，暂不实现）：
- 阶段 2 评估把 Codex 从"每轮一个 `exec`"换成 `codex app-server`（先做实测）。
- 阶段 2 接 Claude 时参考 Agent SDK 的做法：以会话变为空闲作为一轮结束、固定规则生成会话 id、工具只在启动时传入。
- 阶段 3 手机配对照搬"一次性票据换长期凭证"和"宿主端撤销"。

## 5. 明确不采纳

- 镜像或软链接用户的整套 agent 配置、把钩子装进用户全局配置。Polyclave 只共享 skills（经 cc-switch），其余隔离。
- 调用各家的私有用量接口、动用"重置额度"、一个订阅用到上限就自动换账号、对旧版 `agy` 冒充其他客户端——这些都有条款风险。额度继续只用 Agent HUD 看。
- 推送带正文、经第三方网关转发。
- 由模型负责编排调度。
