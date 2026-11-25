# 多厂商 agent 控制台：现有项目调研与方向总结

- 日期：2026-11-24
- 状态：调研结论。**方向已定（2026-11-24）：结合方案**，即 T3 Code + 总主会话 + 我们自己的宿主服务和 workflow，见当时的 T3 结合方案（未公开）。之后方向又改为自建宿主，见 `docs/superpowers/specs/2026-11-25-agent-console-design-v4.md`。
- 相关文档：当时的控制台总体设计、dev 流程与安全门禁（v5.3）及其计划均为早期设计（未公开）。

## 1. 我们要做什么

一个本机运行的**多厂商 agent 控制台**：

- 在一个界面里使用各家自己的 agent（Claude Code、Codex、DeepSeek Harness、Antigravity 等），因为各家 agent 的内置提示词最适配自家模型；不同角色用不同厂商的模型。
- 每家一个 manager 会话，只出方案、推荐自家模型；控制台总主会话负责分配任务和主持讨论。
- **方案讨论**：设计类任务，每家只出一版方案，互相看不到；总主会话隐去来源后汇总，列给用户拍板。
- **dev 流程**：analyst 细化 plan → worker 实现 → 不同厂商或模型的 reviewer 评审 → 自动合并。安全门禁包括：review 绑定 commit SHA、合并只针对已批准的 head、REVISE 最多 3 轮、冲突找人、暂停时撤销 auto-merge。
- **调度由代码执行**，不让模型之间互相派活。
- 多项目按文件夹分级；skills 全局共享。
- **进化**：本地模型读真实运行记录，自动改进 prompt、skills 和路由表；配有受保护规则、版本回滚和自动回滚。

## 2. 已调研的项目

| 项目 | 定位 | 语言 / 许可证 | 结论 |
|---|---|---|---|
| [T3 Code](https://github.com/pingdotgg/t3code) | 编码 agent 的控制平台：在一个界面里使用 Claude Code、Codex、Antigravity、Cursor、OpenCode 和任意 ACP agent；用用户自己的订阅；每个线程可以有独立 worktree，能看 diff、提交、开 PR；多项目；手机和远程访问 | TypeScript / MIT；7.7 万星，非常活跃 | **最接近我们的目标**，见 §3 |
| [Agent HUD](https://github.com/jazzenchen/agent-hud-open) | 屏幕边缘、手机、手表上的 HUD：各家额度、token 用量、会话状态、审批；内置 MCP，让 agent 之间互发消息（支持 DeepSeek Harness） | Rust + Tauri / Apache-2.0 | **用来看各模型的额度和用量**，与控制台配合使用，不承担调度 |
| [AionUi](https://github.com/iOfficeAI/AionUi) | 多 agent 桌面客户端，可接 20 多种 CLI agent（含 Antigravity） | 界面 TS，引擎 AionCore 是 Rust / Apache-2.0 | 引擎是 Rust，嵌不进我们的 TS 调度；多 agent 由模型调度，成员默认全自动执行；没有 PR/合并门禁。**只借鉴思路** |
| [zeroshot](https://github.com/the-open-engine/zeroshot) | 确定性的图引擎：流程由引擎强制执行，每个节点可以指定 agent 和模型 | Rust / MIT | 理念相同，但是 Rust；review 结论由模型自报、不绑定 SHA；UI 不能对话；协议经常有不兼容变更。**借鉴它交付层的 SHA 校验** |
| [vibe-kanban](https://github.com/BloopAI/vibe-kanban) | 看板 + 每个任务一个 workspace，支持 10 多种 agent | Rust + React / Apache-2.0 | **已宣布停止开发**。借鉴它驱动各家 agent 的方式 |
| [metaswarm](https://github.com/dsifry/metaswarm) | "会自我改进"的多 agent 编排；JSONL 知识库；写代码的总由另一个模型 review | Shell / MIT | 借鉴它的知识库和评审门 |
| [karpathy/llm-council](https://github.com/karpathy/llm-council) | 多个模型各自作答、匿名互评、主席模型汇总 | Python | 印证了"隐去来源"的方案讨论设计 |
| [Harness²](https://github.com/google-research/harness2) | 离线优化 harness 的研究框架 | Python / Apache-2.0 | 借鉴它的经验库格式（cue / lesson / evidence） |

前三个深度对比（AionUi、zeroshot、vibe-kanban）读的是浅克隆的源码；T3 Code 读的是文档和 workflow skill；**都没有实际运行**。

### 各项目值得借鉴的具体做法

- **直接驱动各家 agent**：Claude 用 `claude -p --input-format stream-json --output-format stream-json`，续聊用 `--resume`；Codex 用 `codex app-server`（JSON-RPC）；其他 agent 走 ACP。（vibe-kanban、AionUi）
- **交付层的 SHA 防护**：合并前，重新读取那个确切 head 上的 checks 和 reviews；合并后，确认实际合入的 revision；同一个 push 被拒绝重复出现就停下；PR 上的反馈只采纳有写权限的人。（zeroshot）
- **结构化结论**：reviewer 的结论用 schema 强制输出（Claude `--json-schema`，Codex `--output-schema`）。（zeroshot）
- **启动前探测各家 CLI 的版本**。（zeroshot、AionUi）
- **统一的事件词汇表 + 纯函数 reducer**。（AionUi、vibe-kanban）
- **skills 的分发方式**：每个会话一个软链接视图，再按厂商转换成各自的启动参数。（AionUi）

## 3. T3 Code 为什么可能改变方向

以下**假设 T3 Code 能做到它文档里写的**（文档：`docs/user/workflows.md`、`skills/t3-workflows/SKILL.md`、`docs/internals/01_architecture.md`、`docs/internals/03_providers.md`）。

### 3.1 关键能力：workflow

workflow 是一个 TypeScript 脚本，把 agent、普通代码、定时任务、webhook、issue/PR 事件和"向用户提问"串成一个**持久化的运行**，服务器重启后能从断点继续。主要原语：

- `agent(name, { brief, project, model: { instanceId, model }, workspace: { type: 'worktree' }, runtimeMode, returns: Schema })`：在一个线程里运行指定厂商和模型的 agent。可以要求按 schema 返回结构化结果，格式不对会让 agent 改正，最多 2 次。
- `step`：普通代码步骤，结果会被记录，失败时暂停运行。
- `ask`：向用户提问并等待回答。
- `wait` / `signal`：等待外部信号。
- `Effect.all([...], { concurrency })`：并行执行。
- `invoke` / `start`：启动子 workflow。
- issue/PR 触发器，以及 `issues.comment`、`pullRequests.act(..., 'merge')` 这类操作。

### 3.2 我们的设计和 T3 Code 的对应关系

| 我们的设计 | 在 T3 Code 上怎么实现 |
|---|---|
| 控制台界面、和各会话对话、多项目、worktree、看 diff、开 PR | T3 Code 自带 |
| 各家 agent 接入 | 自带 Claude Code、Codex、Antigravity、ACP；**DeepSeek Harness 需要写一个 ACP 转接层**（把 `dsh headless --json` 包装成 ACP agent），因为 T3 规定非官方 agent 只走标准 ACP |
| 确定性调度 | workflow 本身就是代码，状态持久化，流程由代码决定 |
| 方案讨论 | 一个 workflow：并行调用每家的 `agent` 各出一版方案（schema 约束格式）→ 代码隐去来源 → 总主模型汇总 → `ask` 让用户拍板 |
| dev 流程 | 一个 workflow：analyst 输出结构化 plan → worker 在 worktree 中实现 → 不同模型的 reviewer 并行输出结构化结论 → 最多 3 轮 → 代码步骤校验 SHA、用 review token 提交 review → 合并 |
| 安全门禁（SHA 绑定、只合并已批准的 head、暂停） | 写在 workflow 的代码步骤里，由代码执行 |
| 路由表、经验库 | workflow 的持久化存储，或项目里的文件 |
| 进化循环 | 一个定时 workflow：读运行记录，调用本地模型（例如通过 ACP / OpenCode 接 Ollama），写回路由表和 prompt |
| 额度与用量 | Agent HUD |

### 3.3 这条路线的好处

- 不用自己做界面、多项目、适配层和会话管理；自建方案 1A 的 14 个任务大半不再需要。
- **不 fork、不改 T3 的源码**，只写 workflow 文件和一个 DSH 的 ACP 转接层，所以也不受它"fork 必须换品牌"的约束。
- 还能直接得到手机 App 和远程访问。

### 3.4 风险

- **workflow 还是实验功能**（Labs，默认关闭），项目迭代很快（一周约 100 个提交），接口可能变。
- 代码量很大（约 135 万行 TS），大量使用 Effect-TS；万一 workflow 不够用、必须改它的内部，成本很高。
- DSH 依赖我们自己写的 ACP 转接层；T3 对 ACP 的要求很严格。
- 会放弃 superagent 现有的飞书来源和准入策略引擎；T3 有 issue 触发和 webhook，可以替代大部分。
- "用户在控制台里发的消息才算授权"这条规则，要在 workflow 的触发和启动方式上重新落实。
- 用订阅登录驱动 agent 的条款风险依然存在：Claude 的官方文档禁止第三方产品使用订阅登录，个人使用属于灰色地带。

## 4. 两个候选方向

| | A. 基于 T3 Code（只写 workflow） | B. 自建控制台（现有 1A 计划） |
|---|---|---|
| 工作量 | 小：几个 workflow + DSH 的 ACP 转接层 | 大：1A 共 14 个任务，之后还有界面 1B |
| 控制力 | 受 T3 的能力和接口变动约束 | 完全自主 |
| 安全门禁 | 在 workflow 代码中实现 | 在我们的 core 中实现（已有设计） |
| 界面、手机、远程 | 直接拥有 | 要自己做，暂时没有手机端 |
| 主要风险 | Labs 功能不稳定；DSH 只能走 ACP | 工期长；要自己跟进各家 CLI 的变化 |

当前倾向：**A**，前提是 T3 Code 确实能做到文档里写的。实施前要先做一个最小验证：一个方案讨论 workflow。

**最终选择（2026-11-24）：A 与 B 结合。** Gemini 评审建议选 B，主要理由是纯 workflow 的"启动运行、点选择题"模型，不适合"跟总主会话自由对话、由它派活"。用户希望保留总主会话的对话体验，同时不想自建整个底座，所以采用：
- T3 Code 提供界面、会话和流程引擎；
- 总主会话作为一个常驻线程，通过我们自己的**宿主服务**派活；它可以替用户确认，但必须引用用户本人的消息；
- 门禁和凭据放在宿主服务里，workflow 代码通过接口调用。

另外补充两项事实：DSH 自带 ACP（`dsh --profile acp`），Antigravity 有官方 ACP 入口（`agy acp`）。

## 5. 待决问题

1. 方向选 A 还是 B？如果选 A，superagent 现有代码（core 状态机、v5.3 门禁设计）哪些保留为 workflow 内部的库，哪些放弃？
2. DSH 的 ACP 转接层是否可行：`dsh headless --json` 每次只跑一个 turn，能否满足 ACP 会话的要求（续聊、取消、权限请求）？
3. 在 T3 Code 中，"只有用户的明确请求才开工"这条规则如何落实？
4. 进化循环的运行记录从哪里获取：T3 的线程和 workflow 记录、Agent HUD 的用量统计，还是两者结合？
