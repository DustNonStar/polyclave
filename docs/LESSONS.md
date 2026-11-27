# Polyclave 多 Agent 协作与工程经验库（Lessons Learned）

> 本文档专门沉淀 Polyclave 开发过程中“人工中枢 + 审查 Agent + 分支执行 Agent”协作的实战经验，以及可直接反哺 Polyclave 产品设计的洞察（对应未来 `polyclave.sqlite` `memory_items` 中的 `pitfall` / `decision` 种子数据）。
> - 硬性站立规则见 `AGENTS.md`
> - 逐轮交付流水与验收结论见 `HANDOFF.md`

### 记录模板
- **发生了什么：**
- **根因：**
- **流程改进：**
- **反哺 Polyclave 设计：**

---

## 2026-11-26 · 多 Agent 协作与 v5 架构启动

### 1. 「遇冲突即停 + 结构化 A/B 选项与推荐」是最省返工的协作协议
- **发生了什么：** Dot 在 `dot/v5-phase1a` 动工前及 Task 4 前，先后通过 `PLAN-ISSUES.md` 拦下 3 处冲突：`authorized → go` 之间崩溃与已运行后退出的区分、`rusqlite` 无独立 `fts5` feature、v4 旧测试（取消冻结/复用旧 worktree）与 v5 Attempt 隔离的冲突，并给出带推荐项的 A/B 选项。
- **根因：** 实施计划在跨大版本迁移时未预先列出“旧测试语义变更白名单”，且部分底层边界（如 `authorized → go` 窗口）只有在真正写代码/跑校验时才会暴露。
- **流程改进：** 后续编写跨版本实施计划时，显式附上「旧测试语义变更白名单」；执行 Agent 遇到计划未授权的冲突时继续坚持“停下 → 写证据 → 给 A/B 选项与推荐”。
- **反哺 Polyclave 设计：** 阶段 1B 可为 Worker 新增结构化 MCP 工具 `raise_plan_issue({ level: 'detail' | 'direction', evidence, options, recommendation })`，`detail` 级由主控 `hub` 自动裁决放行，`direction` 级才弹窗请用户拍板。

### 2. 设计稿、规范文字与无头渲染预览的三方漂移
- **发生了什么：** 核查 `polyclave-v5-desktop-ui.fig` 时发现：底层线性渐变漏设垂直方向矩阵导致全局底部极光出现水平切线；`Deepy` 像素蓝鲸本身无脚，但规范文字笼统写成了“4 细腿”；Node 无头 CanvasKit 渲染因未注册系统 CJK 字体而隐藏了中文节点。
- **根因：** 视觉稿、设计文档和无头截图工具各自独立演进，缺少一步交叉核对。
- **流程改进：** 每次修改设计稿或规范后执行“图层 ↔ 规范文字 ↔ 实施计划”三方交叉核对；涉及中文字体视觉验收时，直接用原生桌面工具（OpenPencil）目视终审，不以无头截图作为唯一依据。
- **反哺 Polyclave 设计：** 阶段 1B 的无头浏览器验收门禁必须实测中文字体渲染与全局视口连续性，不能仅靠 DOM 断言。

### 3. 多 Agent 并行推进时的「零冲突分支纪律」
- **发生了什么：** Dot 在 `dot/v5-phase1a` 上实施阶段 1A 底座期间，我们在本地 `main` 补充了阶段 3（`google/agentmesh`）、阶段 4（社区工作流蒸馏）设计及经验库，但刻意不触碰 1A 计划文件且暂缓推送 `main`。
- **根因：** 若在执行 Agent 拉出工作分支后频繁往 `main` 推送或改动其正在执行的计划文件，会制造无谓的 rebase/merge 冲突。
- **流程改进：** 分支执行期间，`main` 只做不重叠的远期文档演进并暂留本地；待分支审查合并后再统一合流推送。
- **反哺 Polyclave 设计：** 主控 `hub` 同时调度多个 Worker 时，对同一仓库的共享契约/文档变更应串行化或在独立整合窗口合流。

### 4. 「云端确定性闭环 + 本机实机门禁」的分层验证
- **发生了什么：** Dot 在云端 Linux 环境跑通了确定性离线测试与 macOS/Windows 交叉编译及 Clippy，同时明确标注真实双平台运行与 `POLYCLAVE_REAL_CODEX/CLAUDE` 探针需留待本机合并审查时实跑。
- **根因：** 云端分支执行环境缺乏用户本机真实 CLI 登录态与双平台真机环境，硬凑反而容易引入虚假 mock 或泄露凭据。
- **流程改进：** 明确分层验收边界：分支执行 Agent 负责确定性离线闭环与 CI 编译/静态检查；合并审查者负责本机双平台实跑、真实 CLI 探针与 UI 实测。
- **反哺 Polyclave 设计：** 与 Polyclave “Worker 在 `network_access=false` 沙箱跑确定性闭环，宿主在可信本机执行 R1 核证与最终门禁”的架构完全一致。
