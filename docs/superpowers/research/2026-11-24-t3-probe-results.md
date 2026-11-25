# T3 Code 实测记录（三轮）

- 日期：2026-11-24
- 对象：结合方案 draft v2 §11 的验证清单
- 前提（用户决定）：**T3 和你用同一个 macOS 用户运行**，不拆分到专用用户。
- 环境：T3 nightly `0.1.12-nightly.20261124.5209`（workflow 只在 nightly 里有；稳定版 0.1.11 没有 `t3 workflow` 命令），用 `npx` 下载，`t3 serve --base-dir <scratchpad>` 跑一个隔离实例，没有动 `~/.t3` 里的配置；下载的运行文件在 `~/.t3/runtime/versions/`（两个版本，各约 400MB）。
- 实测用的是模拟：从一个普通 shell（相当于一个有命令行权限的 agent）去操作 T3。

## 1. 结论摘要

| # | 验证项 | 结果 |
|---|---|---|
| 2 | 同用户进程能否冒充你发消息 | **能**。不需要任何确认 |
| 3 | 同用户进程能读到什么 | workflow 参数明文存在 SQLite；T3 的签名密钥、会话；同一用户下已登录的 `gh`（若有） |
| — | 同用户进程能否回答 workflow 的提问 | **能**。`t3 workflow signal` 不需要认证 |
| 4 | 宿主侧 reviewer 的结构化结论 | Antigravity 可以；DSH 没有 schema 参数；Claude 本次因认证未配置好未测通 |
| 5 | workflow 基本能力 | `step`、`ask`、CLI 启动和回答都正常；`agent` 步骤因各家 key 不可用未测 |

## 2. 细节

### 2.1 冒充你发消息（决定 §5 派活规则是否成立）

1. 只要设置 `T3CODE_HOME`，同用户的任何进程都能直接用 `t3` CLI 操作正在运行的服务器（列线程、开线程、发消息、启动 workflow），不需要配对或令牌。
2. CLI 开的线程被标为 `createdBy = agent`、`creationSource = mcp`，这一点 T3 是诚实的。
3. 但 `t3 auth session issue` 可以**免确认**签发一个完整权限的 bearer 令牌（有效期 30 天，scope 包括 `orchestration:operate`、`filesystem:write`、`access:write` 等）。
4. 拿着令牌走 WebSocket，以普通客户端身份发消息时，服务器把 `createdBy` 固定写成 `user`，`creationSource` 用客户端自己报的值（源码 `apps/server/src/ws.ts`：`createdBy: "user"`, `creationSource: input.creationSource ?? "web"`）。

**所以在同一用户下，"依据必须是你本人在网页或手机上发的消息"无法用代码保证**：被注入的 agent 只要能执行命令，就能造出一条完全合格的"你的消息"。（第 4 步是读源码得出的，没有实际发 WebSocket 消息。）

### 2.2 workflow 参数与提问

- 用一个带 canary 字符串的参数启动 workflow：`t3 workflow show` 不显示参数，但参数**明文**存在 `userdata/statev2.sqlite` 和它的 WAL 文件里，同用户可读。
- `t3 workflow questions` 能列出所有等待回答的问题，`t3 workflow signal '{"choice":"Yes"}' --run <id> --wait go` 能直接回答，不需要认证。**升级给你的问题，被注入的 agent 也能替你回答。**

### 2.3 本机凭据

- T3 的 `userdata/secrets/`（服务器签名密钥、MCP OAuth 签名密钥等）权限 600，但属于你的用户，同用户可读。
- 本机 `gh` 如果登录着个人账号（带 `repo` 等权限），同用户的 agent 可以直接用它 approve、合并、甚至修改分支保护。

### 2.4 宿主侧 reviewer CLI

| CLI | 无交互 + 结构化输出 | 实测 |
|---|---|---|
| Antigravity `agy -p --output-format json --json-schema` | 有 | **通过**：返回 `structured_output` 字段。注意：`--mode plan` 下不产出结构化结果，而且会拒绝 Bash 等工具；把 diff 直接放进提示词、不用工具即可 |
| Claude `claude -p --output-format json --json-schema` | 有 | **未测通**：T3 里和命令行里都报 `billing_error`（当时的认证未配置好） |
| DSH `dsh --profile headless --json` | **没有 schema 参数** | **未测通**：headless profile 里没有 `DEEPSEEK_API_KEY`；`--permission-mode` 需要先配置权限预设。要输出结构化结论只能靠提示词，再由宿主校验 |
| Codex | — | 本机没有安装 `codex` |

### 2.5 未测

- workflow 的 `agent` 步骤、manager 线程并发、`resume` 是否重放副作用：都需要可用的 agent 凭据。
- `dsh --profile acp` / `agy acp` 在 T3 里的稳定性：同上。T3 里 Antigravity 需要单独配置 API key。
- Antigravity 在 T3 私有 `GEMINI_HOME` 下读取全局 skills。

## 3. 对方案的影响

同一用户下，**T3 里的任何东西都挡不住一个能执行命令的 agent**：派活依据、workflow 的提问、workflow 参数都能被伪造或读取。v2 里依赖"代码核对你的消息"的部分（§5）只能防住"主会话被说服但不执行命令"这一类情况。

真正还守得住的只有 T3 之外的东西：

1. **GitHub token 必须不在你的用户下**：review token、merge token 放进单独的宿主用户（这个和 T3 跑在哪个用户无关，仍然可行）。
2. **本机登录着个人账号的 `gh` 是最大的漏洞**：只要有，任何 agent 都能用你的身份 approve、合并、改分支保护，其他门禁都形同虚设。

需要你决定的事项见对话。

## 4. 用户的决定（2026-11-24）

用户认为上面的担心过度：控制台和 Claude Code 一样是本地客户端，本机 agent 按同样的标准信任；如果有人能拿到本机令牌，本来就什么都能拿到。因此：

- 只用一个 macOS 账户，不做隔离。
- "你发的消息"用客户端标记识别：直接使用 T3 自带的 `createdBy = user` + `creationSource = web/mobile`。它能区分你在客户端里发的消息和 agent、其他线程、工具转来的内容；不防本机进程故意伪造，这在信任模型之内。
- 门禁只防出错。见结合方案 draft v3。

## 5. 第二轮实测（2026-11-25）

- 环境：T3 桌面端 Nightly `0.1.12-nightly.20261125.5230`，数据在 `~/.t3`；正式版 0.1.12 也已带 workflow（Labs 功能，需在 Settings → Labs 打开）。已关闭 T3 的产品遥测（在登录 shell 环境中设置 `T3CODE_TELEMETRY_ENABLED=false`，重启后 T3 的环境缓存为 `false`）。
- 凭据：各家用本机已配置的命令行（Codex 0.172.0）；DSH 尚未添加。
- 测试项目：临时目录里的 git 仓库 `t3smoke`（只有一个 README）。

| 验证项 | 结果 |
|---|---|
| 各家单条消息 | Claude（`claude-haiku-5-5`）、Codex（`gpt-6-luna`）、Antigravity（`gemini-3.8-flash`）都正常。Antigravity 实际可用的模型名和 model-manifest 文件不一致（如没有 `gemini-3.8-flash-low`） |
| workflow `agent` 并行 + schema | 通过。三家都按 schema 返回 |
| `thread` 继续已有线程 | 通过 |
| schema 一直不交 | 纠正两次后放弃；`agent.try` 返回 `failed: the agent did not return a result` |
| 代码步骤失败 → `resume` | 只重跑失败的步骤；之前完成的有副作用步骤没有重跑（计数文件只有一行） |
| 等待提问时重启 T3 → 回答 | 运行接着往下走；已完成的有副作用步骤没有重放（计数文件共两行） |
| plan 模式能否只读 | **不能**。明确要求改 README 时，Codex 和 Antigravity 在 plan 模式下都改了文件（线程是 `full-access` + `plan`）；Claude 按 plan 模式的指令自觉拒绝 |
| `approval-required` | Codex 读文件也要等批准，workflow 一直卡在 waiting；Claude 不受影响 |
| 用时 | plan 模式下三家 reviewer 各 33～42 秒 |
| 消息来源标记 | 数据库里：导入的 Claude Code 历史会话 `user` + `server`（526 条）；workflow agent 线程 `system` + `server`；CLI 线程 `agent` + `mcp`。还没有从桌面端发出的消息，桌面端的来源值待测 |
| 宿主读取标记 | `t3 thread show --json` 只有线程级的 `createdBy`/`creationSource`，消息只有 `role`。MCP `t3_thread_read` 逐条返回（读源码）。`/mcp` 不认 `t3 auth session issue` 的令牌，外部客户端要用 OAuth 登录（可选权限级别）。`t3 auth session issue --scope orchestration:read` 可以签发只读令牌 |
| Antigravity 的 skills | T3 私有目录里 `config/skills` 链接到 `~/.gemini/config/skills`，能读到全局 skills |
| `t3 thread start --output-schema` | 存在：不经 workflow 也能让一个线程按 schema 返回 |

对方案的影响（已写入 draft v3.1）：
1. reviewer 不能靠 T3 的模式做到只读，改为在一次性 worktree 里运行，宿主提交 review 前核对 PR head。
2. "你的标记"必须同时检查 `creationSource`，只看 `createdBy = user` 会把导入的历史会话也算进去。
3. 宿主以 MCP OAuth 客户端（只读）身份读取消息来源。
4. 正式版 0.1.12 起就有 workflow，不必用 nightly。

## 6. 第三轮：不经 T3 直接调用（2026-11-25）

- 目的：用户决定不以 T3 为底座，只需证明各家 agent 能被我们自己的程序直接调用并返回结构化结果。T3 的其余验证项（桌面端消息来源、OAuth 调 `t3_thread_read`、manager 线程并发、DSH 在 T3 里的稳定性、合并测试）**不再做**。
- 做法：同一个提示词（给出一行 README，要求不用工具，返回 APPROVE/REVISE 和理由），同一个 JSON schema（`verdict` 枚举 + `reason`），四家并行各调一次。

| Agent | 命令 | 结果 |
|---|---|---|
| Claude（`claude-haiku-5-5`） | `claude -p --model … --output-format json --json-schema "<schema>"`（去掉 `ANTHROPIC_API_KEY`） | **通过**。`structured_output` 符合 schema，约 5.6 秒 |
| Codex | `codex exec --skip-git-repo-check -s read-only --output-schema schema.json -o last.json` | **通过**。`-o` 文件符合 schema |
| Antigravity（官方 `agy`） | `agy --output-format json --json-schema "<schema>" -p="<prompt>"` | **通过**。`structured_output` 符合 schema，约 11 秒。注意 `-p` 必须用 `-p=` 紧贴提示词，否则会把下一个参数当成提示词 |
| DSH | `dsh headless --json "<prompt>"` | **未跑通**：`MISSING_CREDENTIAL`，缺 `DEEPSEEK_API_KEY`（可在 DSH 网页端 Models 页面或环境变量里设置）。DSH 没有 schema 参数，只能靠提示词 + 调用方校验 |

- 同时核对了各家续接会话、注入 MCP、权限参数（见 v4 方案 §6.5）。
- 结论：直接调用可行，方案改为 v4（`docs/superpowers/specs/2026-11-25-agent-console-design-v4.md`）。

## 7. T3 测试总结

| 结论 | 来源 |
|---|---|
| 同一 macOS 用户下，任何进程都能操作 T3（发消息、签令牌、回答 workflow 提问）；用户决定按"本机 agent 可信"处理 | 第一轮 |
| workflow 的 `agent` 并行、schema 返回、续接线程都正常 | 第二轮 |
| schema 不交：纠正两次后放弃；`resume` 只重跑失败步骤；等待提问时重启不重放 | 第二轮 |
| plan 模式不能保证只读；`approval-required` 会让 Codex 卡住 | 第二轮 |
| 导入的历史会话也是 `createdBy = user`，只看这个字段会误判 | 第二轮 |
| T3 默认开启产品遥测（已关闭）；手机推送要经 T3 的云 | 第二轮及源码 |
| 不经 T3 也能直接调用 Claude、Codex、Antigravity 并按 schema 返回 | 第三轮 |

## 8. 宿主闭环：接管一个 Codex 会话（2026-11-25，不涉及 T3）

- 目的：证明宿主能稳定接管会话，而不只是单次拿到 JSON。只接 Codex 一家。
- 脚本：`docs/superpowers/research/probes/codex-host-loop.mjs`（Node 26，`node:sqlite`，自带一个最小的 streamable HTTP MCP 服务，只有 `submit_result` 一个工具，宿主按 schema 校验）。每次 `send` 都是一个新的宿主进程。
- Codex 0.172.0；每轮一个 `codex exec --json` 进程，之后用 `codex exec resume <id>` 续接；MCP 用 `-c mcp_servers.host.*` 注入，令牌经环境变量 `bearer_token_env_var` 传入。

| 轮 | 做什么 | 结果 |
|---|---|---|
| 1 | 新会话，记住暗号 ORCHID-7 | 通过，会话 id 写入 SQLite（7 秒） |
| 2 | 新宿主进程续接，要求调用 `submit_result` | 续接成功，暗号正确；工具调用失败：`MCP tool call requires approval, but approval policy is never`。还调用了用户全局配置里的 hindsight MCP |
| 3 | 加 `default_tools_approval_mode="approve"`（只对 host 服务器）和 `--ignore-user-config` 后续接 | **通过**：`{"codeword":"ORCHID-7","turn":3}` 写入 `results` 表 |
| 4 | 故意先交 `orchid 7` | 宿主拒收（返回原因），Codex 改为 `ORCHID-7` 重交，通过；两条都记入 SQLite |
| 5 | 运行中 `kill -9` 宿主 | Codex 子进程没有退出，继续跑完；MCP 调用落空；SQLite 只有用户消息 |
| 6 | 新宿主续接 | 通过，`submit_result` 写入成功（Codex 自述上一轮在提交前被中断） |

结论：
1. "发消息 → 用会话 id 续接 → `submit_result` → 写入 SQLite"的闭环成立，跨宿主重启也成立。
2. 宿主必须隔离用户的全局 agent 配置，并只对宿主工具免批准。
3. 宿主必须自己管理子进程（进程组、启动时清理残留），否则崩溃后会有孤儿进程和新进程同时操作同一个会话。
4. 以上已写入 v4 方案 §6.8、§7.5。

## 9. 崩溃恢复：v4 第一版验收目标（2026-11-25）

- 目标：宿主在任务进行中被强制关闭，重启后既不重复执行外部动作，也不把没做完的事标成成功。
- 脚本：`docs/superpowers/research/probes/codex-crash-recovery.mjs`。一个本地裸仓库 `remote.git` 代替 GitHub；任务是"建分支 `task-<run>`、提交一个内容固定的文件、推送、调用 `submit_result` 报告 SHA"。宿主校验时会去远端核对 SHA。
- 宿主每次启动先做恢复：
  1. 找出上次留下的"进行中"轮次，按记录的进程组杀掉残留的 Codex；
  2. 对中断的任务先查远端：没有分支就可以重试；分支存在且内容、提交数都符合预期，就直接采纳并标记来源为 `reconciled`，**不重试**；分支存在但内容不对，标为 `needs_review`，不再动它。
- 同一会话加锁：已有一轮在跑时，第二个宿主拒绝执行。

| 场景 | 结果 |
|---|---|
| r1 正常 | 完成。第一次跑时发现 Codex 的 `workspace-write` 沙箱把 `.git` 设成只读，Codex **自己绕道**去临时目录克隆后推送；把 `work/.git` 加进 `writable_roots` 后正常 |
| r2 同一会话同时启动两个宿主 | 第二个返回 BUSY（退出码 3），第一个正常完成 |
| r3 宿主被杀（Codex 留下来继续跑） | 重启时孤儿进程已经推送完；宿主杀掉孤儿，查到远端分支，采纳，不重试 |
| r4 宿主和 Codex 在"第一条命令开始"时一起被杀 | Codex 一条命令就完成了提交和推送，被杀前已推送；重启后采纳。另外这一轮 Codex 先试图报告上一个任务（r3）的结果，宿主因分支不对拒收 |
| r5 宿主和 Codex 在这一轮刚开始时一起被杀 | 远端没有分支，重试；最终 1 个提交 |
| r6 本地已有提交但没推送（模拟） | 重试时 Codex 复用了已有的本地提交（SHA 相同），远端 1 个提交 |
| r7 推送成功后、提交结果前宿主被杀 | 重启杀掉孤儿，查到远端分支，采纳，不重试，1 个提交 |
| r8 远端分支内容不对（模拟） | 标为 `needs_review`，两次启动都不动它 |
| 结束时 | 8 个分支各 1 个提交；没有残留的 Codex 进程 |

结论与要求（已写入 v4 方案 §7.5、§8.2、§17.1）：
1. **验收目标达到**：所有场景都没有重复的外部动作；没完成的任务没有被标成成功；无法确定的标为 `needs_review`。
2. **宿主无法在一条命令内部插手**：agent 常把提交和推送写在同一条 shell 命令里。所以不能靠"在动作前拦住"，只能靠事后对照外部状态。外部动作必须带上能查回来的标识（分支名、PR 正文里的运行 id）。
3. **结果必须绑定任务 id**：被中断的 agent 在下一轮会去补报上一个任务的结果。
4. **沙箱要把 git 元数据设为可写**（普通仓库的 `.git`；worktree 还要加上主仓库里的 `.git/worktrees/<name>` 和对象目录），否则 agent 会自己绕道，结果更难预测。
5. **`--ignore-user-config` 挡不住 `~/.codex/skills`**：Codex 仍会读你的全局 skills，宿主要另外处理（例如单独的 `CODEX_HOME`）。
6. 本脚本的会话锁是"先查再写"，两个宿主几乎同时启动时仍可能都通过；正式实现要用数据库唯一约束（同一会话只允许一行"进行中"）。
