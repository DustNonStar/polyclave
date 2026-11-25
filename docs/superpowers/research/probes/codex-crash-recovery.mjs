// Crash-recovery probe: a Codex task with an external side effect (push a branch to a
// local bare repo standing in for GitHub). The host may be SIGKILLed mid-turn. On every
// start the host (1) kills leftover agent process groups, (2) reconciles interrupted runs
// against the external system before deciding to adopt, retry, or flag for review.
// Usage:
//   node crash.mjs setup
//   node crash.mjs run <runId> [--crash=before-push|after-push]
//   node crash.mjs status
import { DatabaseSync } from "node:sqlite";
import { createServer } from "node:http";
import { spawn, execFileSync } from "node:child_process";
import { randomBytes } from "node:crypto";
import { mkdirSync, rmSync, existsSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
// Each probe keeps its state in its own ignored directory so the two never delete each other's files.
const STATE = join(HERE, ".state-codex-crash-recovery");
mkdirSync(STATE, { recursive: true });
const REMOTE = join(STATE, "remote.git");
const WORK = join(STATE, "work");
const SESSION = "worker-1";
const git = (cwd, ...a) => execFileSync("git", a, { cwd, encoding: "utf8" }).trim();
const now = () => new Date().toISOString();
const log = (...a) => console.error(`[host ${process.pid}]`, ...a);

const db = new DatabaseSync(join(STATE, "host.sqlite"));
db.exec(`
  PRAGMA journal_mode = WAL;
  CREATE TABLE IF NOT EXISTS sessions (name TEXT PRIMARY KEY, agent_session_id TEXT, mcp_token TEXT NOT NULL);
  CREATE TABLE IF NOT EXISTS runs (
    id TEXT PRIMARY KEY, session TEXT NOT NULL,
    status TEXT NOT NULL,          -- running | interrupted | done | needs_review
    detail TEXT, updated_at TEXT);
  CREATE TABLE IF NOT EXISTS turns (
    id INTEGER PRIMARY KEY, run TEXT NOT NULL, session TEXT NOT NULL,
    status TEXT NOT NULL,          -- running | completed | interrupted
    host_pid INTEGER NOT NULL, agent_pid INTEGER, started_at TEXT, ended_at TEXT);
  CREATE TABLE IF NOT EXISTS results (
    id INTEGER PRIMARY KEY, run TEXT NOT NULL, accepted INTEGER NOT NULL,
    source TEXT NOT NULL,          -- agent | reconciled
    payload TEXT NOT NULL, error TEXT, at TEXT NOT NULL);
  CREATE TABLE IF NOT EXISTS log (id INTEGER PRIMARY KEY, at TEXT, host_pid INTEGER, text TEXT);
`);
const note = (text) => { log(text); db.prepare("INSERT INTO log (at, host_pid, text) VALUES (?, ?, ?)").run(now(), process.pid, text); };
const setRun = (id, status, detail = null) =>
  db.prepare("UPDATE runs SET status = ?, detail = ?, updated_at = ? WHERE id = ?").run(status, detail, now(), id);

// ---- process helpers ----
const alive = (pid) => { try { process.kill(pid, 0); return true; } catch { return false; } };
const cmdOf = (pid) => { try { return execFileSync("ps", ["-o", "command=", "-p", String(pid)], { encoding: "utf8" }).trim(); } catch { return ""; } };
const isOurHost = (pid) => alive(pid) && /crash\.mjs run/.test(cmdOf(pid));
const isCodex = (pid) => alive(pid) && /codex/.test(cmdOf(pid));
async function killGroup(pgid) {
  try { process.kill(-pgid, "SIGTERM"); } catch { return; }
  for (let i = 0; i < 20 && alive(pgid); i++) await new Promise((r) => setTimeout(r, 100));
  try { process.kill(-pgid, "SIGKILL"); } catch {}
}

// ---- external system checks (the bare repo stands in for GitHub) ----
const expectedToken = (run) => `token-${run}`;
function remoteBranch(run) {
  const out = git(HERE, "ls-remote", REMOTE, `refs/heads/task-${run}`);
  return out ? out.split(/\s+/)[0] : null;
}
function remoteFile(sha, run) {
  try { return git(REMOTE, "show", `${sha}:task-${run}.txt`); } catch { return null; }
}
const commitsOnBranch = (run) =>
  Number(git(REMOTE, "rev-list", "--count", `main..task-${run}`));

// ---- step 1 of every start: recover ----
async function recover() {
  for (const t of db.prepare("SELECT * FROM turns WHERE status = 'running'").all()) {
    if (t.host_pid !== process.pid && isOurHost(t.host_pid)) continue; // another live host owns it
    if (t.agent_pid && isCodex(t.agent_pid)) {
      note(`recover: turn ${t.id} left an orphan agent process group ${t.agent_pid}; killing it`);
      await killGroup(t.agent_pid);
      note(`recover: orphan ${t.agent_pid} ${alive(t.agent_pid) ? "STILL ALIVE" : "gone"}`);
    }
    db.prepare("UPDATE turns SET status = 'interrupted', ended_at = ? WHERE id = ?").run(now(), t.id);
    setRun(t.run, "interrupted", `host ${t.host_pid} died during turn ${t.id}`);
  }
  for (const r of db.prepare("SELECT * FROM runs WHERE status = 'interrupted'").all()) {
    const sha = remoteBranch(r.id);
    if (!sha) { note(`recover: run ${r.id}: branch not on remote -> safe to retry`); continue; }
    const content = remoteFile(sha, r.id);
    const n = commitsOnBranch(r.id);
    if (content === expectedToken(r.id) && n === 1) {
      db.prepare("INSERT INTO results (run, accepted, source, payload, at) VALUES (?, 1, 'reconciled', ?, ?)")
        .run(r.id, JSON.stringify({ branch: `task-${r.id}`, sha }), now());
      setRun(r.id, "done", `adopted from remote: branch task-${r.id} at ${sha} (agent never reported)`);
      note(`recover: run ${r.id}: push already happened (${sha.slice(0, 8)}); adopted, NOT retried`);
    } else {
      setRun(r.id, "needs_review", `remote branch exists but content=${JSON.stringify(content)} commits=${n}`);
      note(`recover: run ${r.id}: remote state unexpected -> needs_review`);
    }
  }
}

// ---- host MCP (submit_result) ----
let currentRun = null;
function validate(a, run) {
  if (!a || typeof a !== "object") return "arguments must be an object";
  if (a.branch !== `task-${run}`) return `branch must be task-${run}`;
  if (typeof a.sha !== "string" || !/^[0-9a-f]{40}$/.test(a.sha)) return "sha must be a full 40-char hex commit id";
  const remote = remoteBranch(run);
  if (remote !== a.sha) return `remote task-${run} is at ${remote ?? "nothing"}, not ${a.sha}; push first`;
  return null;
}
function startMcp(token) {
  const server = createServer(async (req, res) => {
    if (req.url !== "/mcp") return res.writeHead(404).end();
    if (req.method !== "POST") return res.writeHead(405, { Allow: "POST" }).end();
    if ((req.headers.authorization || "") !== `Bearer ${token}`) return res.writeHead(401).end();
    let body = ""; for await (const c of req) body += c;
    const msg = JSON.parse(body);
    if (msg.id === undefined) return res.writeHead(202).end();
    const reply = (result) => res.writeHead(200, { "Content-Type": "application/json" })
      .end(JSON.stringify({ jsonrpc: "2.0", id: msg.id, result }));
    if (msg.method === "initialize") return reply({ protocolVersion: msg.params?.protocolVersion ?? "2025-06-18", capabilities: { tools: {} }, serverInfo: { name: "host", version: "0" } });
    if (msg.method === "ping") return reply({});
    if (msg.method === "tools/list") return reply({ tools: [{
      name: "submit_result",
      description: "Report the pushed branch and commit sha to the host. The host checks the remote.",
      inputSchema: { type: "object", properties: { branch: { type: "string" }, sha: { type: "string" } }, required: ["branch", "sha"], additionalProperties: false },
    }] });
    if (msg.method === "tools/call") {
      const a = msg.params?.arguments; const err = validate(a, currentRun);
      db.prepare("INSERT INTO results (run, accepted, source, payload, error, at) VALUES (?, ?, 'agent', ?, ?, ?)")
        .run(currentRun, err ? 0 : 1, JSON.stringify(a), err, now());
      note(`submit_result: ${err ? "REJECTED " + err : "accepted"} ${JSON.stringify(a)}`);
      return reply({ content: [{ type: "text", text: err ? `Rejected: ${err}` : "Accepted." }], isError: !!err });
    }
    return res.writeHead(200, { "Content-Type": "application/json" }).end(JSON.stringify({ jsonrpc: "2.0", id: msg.id, error: { code: -32601, message: "no" } }));
  });
  return new Promise((r) => server.listen(0, "127.0.0.1", () => r(server)));
}

// ---- one turn ----
function brief(run, resumed) {
  const task = `In the git repository in the current directory: create branch task-${run} from main, ` +
    `add a file task-${run}.txt whose entire content is exactly ${expectedToken(run)} (no newline needed), ` +
    `commit it with message "task ${run}", and push it with: git push origin task-${run}. ` +
    `Then call the submit_result tool of the host MCP server with {"branch": "task-${run}", "sha": <the pushed commit sha>}. ` +
    `Do nothing else. Finally reply with only: done`;
  return resumed
    ? `The host was restarted and your previous turn was interrupted. The branch task-${run} is NOT on the remote yet. ` +
      `Check the local repository state first (a local branch or commit may already exist; reuse it rather than creating a second commit). Task: ${task}`
    : task;
}

async function runTurn(run, crash) {
  let s = db.prepare("SELECT * FROM sessions WHERE name = ?").get(SESSION);
  if (!s) {
    db.prepare("INSERT INTO sessions (name, mcp_token) VALUES (?, ?)").run(SESSION, randomBytes(24).toString("hex"));
    s = db.prepare("SELECT * FROM sessions WHERE name = ?").get(SESSION);
  }
  // session lock: refuse if another live host has a running turn on this session
  const busy = db.prepare("SELECT * FROM turns WHERE session = ? AND status = 'running'").get(SESSION);
  if (busy) { note(`BUSY: session ${SESSION} has running turn ${busy.id} (host ${busy.host_pid}); refusing`); process.exit(3); }
  const prev = db.prepare("SELECT * FROM runs WHERE id = ?").get(run);
  if (prev?.status === "done") { note(`run ${run} already done (${prev.detail ?? "agent reported"}); nothing to do`); return; }
  if (prev?.status === "needs_review") { note(`run ${run} needs review: ${prev.detail}; not touching it`); return; }
  const resumed = prev?.status === "interrupted";
  if (!prev) db.prepare("INSERT INTO runs (id, session, status, updated_at) VALUES (?, ?, 'running', ?)").run(run, SESSION, now());
  else setRun(run, "running");
  const turn = db.prepare("INSERT INTO turns (run, session, status, host_pid, started_at) VALUES (?, ?, 'running', ?, ?)")
    .run(run, SESSION, process.pid, now()).lastInsertRowid;

  currentRun = run;
  const server = await startMcp(s.mcp_token);
  const port = server.address().port;
  const common = ["--json", "--skip-git-repo-check", "--ignore-user-config",
    "-c", 'approval_policy="never"',
    "-c", 'sandbox_mode="workspace-write"',
    "-c", `sandbox_workspace_write.writable_roots=["${REMOTE}", "${join(WORK, ".git")}"]`,
    "-c", `mcp_servers.host.url="http://127.0.0.1:${port}/mcp"`,
    "-c", 'mcp_servers.host.bearer_token_env_var="HOST_MCP_TOKEN"',
    "-c", 'mcp_servers.host.default_tools_approval_mode="approve"'];
  const text = brief(run, resumed);
  const args = s.agent_session_id ? ["exec", "resume", ...common, s.agent_session_id, text] : ["exec", ...common, text];
  const child = spawn("codex", args, { cwd: WORK, detached: true, stdio: ["ignore", "pipe", "pipe"],
    env: { ...process.env, HOST_MCP_TOKEN: s.mcp_token } });
  db.prepare("UPDATE turns SET agent_pid = ? WHERE id = ?").run(child.pid, turn);
  note(`turn ${turn} run ${run}: ${s.agent_session_id ? "resume " + s.agent_session_id : "new session"}, agent pgid ${child.pid}${crash ? ", will crash " + crash : ""}`);

  let buf = "", reply = "", stderr = "";
  child.stderr.on("data", (d) => (stderr += d));
  child.stdout.on("data", (d) => {
    buf += d; let i;
    while ((i = buf.indexOf("\n")) >= 0) {
      const line = buf.slice(0, i).trim(); buf = buf.slice(i + 1);
      let ev; try { ev = JSON.parse(line); } catch { continue; }
      if (ev.type === "thread.started" && !s.agent_session_id) {
        db.prepare("UPDATE sessions SET agent_session_id = ? WHERE name = ?").run(ev.thread_id, SESSION);
        s.agent_session_id = ev.thread_id;
      }
      if (crash === "all-at-start" && ev.type === "turn.started") {
        note("SIMULATED CRASH of host AND agent as soon as the turn starts");
        try { process.kill(-child.pid, "SIGKILL"); } catch {}
        process.kill(process.pid, "SIGKILL");
      }
      const it = ev.item;
      if (it?.type === "command_execution") {
        if (ev.type === "item.completed") log(`  cmd exit=${it.exit_code}: ${String(it.command).slice(0, 140)}`);
        if (crash === "before-push" && ev.type === "item.started") { note("SIMULATED CRASH before any command runs"); process.kill(process.pid, "SIGKILL"); }
        // whole-machine style crash: agent and host die together before anything is pushed
        if (crash === "all-before-push" && ev.type === "item.started") {
          note("SIMULATED CRASH of host AND agent before any command runs");
          try { process.kill(-child.pid, "SIGKILL"); } catch {}
          process.kill(process.pid, "SIGKILL");
        }
        // crash in the middle of local work: after a local commit exists, before the push
        if (crash === "all-after-commit" && ev.type === "item.completed" && /git commit/.test(it.command) && !/git push/.test(it.command)) {
          note("SIMULATED CRASH of host AND agent after a local commit, before push");
          try { process.kill(-child.pid, "SIGKILL"); } catch {}
          process.kill(process.pid, "SIGKILL");
        }
        if (crash === "after-push" && ev.type === "item.completed" && /git push/.test(it.command) && it.exit_code === 0) {
          note("SIMULATED CRASH right after a successful git push, before submit_result"); process.kill(process.pid, "SIGKILL");
        }
      }
      if (it?.type === "agent_message" && ev.type === "item.completed") reply = it.text;
    }
  });
  const code = await new Promise((r) => child.on("close", r));
  server.close();
  db.prepare("UPDATE turns SET status = 'completed', ended_at = ? WHERE id = ?").run(now(), turn);
  const ok = db.prepare("SELECT * FROM results WHERE run = ? AND accepted = 1").get(run);
  if (ok && remoteBranch(run) === JSON.parse(ok.payload).sha) setRun(run, "done", "agent reported, remote verified");
  else setRun(run, "interrupted", `turn ended (exit ${code}) without a verified result`);
  note(`turn ${turn} finished exit=${code} reply=${JSON.stringify(reply)}${code ? " stderr=" + stderr.slice(-300) : ""}`);
}

function setup() {
  for (const p of [REMOTE, WORK, join(STATE, "host.sqlite"), join(STATE, "host.sqlite-wal"), join(STATE, "host.sqlite-shm")]) rmSync(p, { recursive: true, force: true });
  execFileSync("git", ["init", "--bare", "-b", "main", REMOTE]);
  execFileSync("git", ["clone", "-q", REMOTE, WORK]);
  git(WORK, "config", "user.name", "probe"); git(WORK, "config", "user.email", "probe@example.invalid");
  git(WORK, "checkout", "-q", "-b", "main");
  execFileSync("sh", ["-c", "echo '# crash probe' > README.md"], { cwd: WORK });
  git(WORK, "add", "."); git(WORK, "commit", "-q", "-m", "init"); git(WORK, "push", "-q", "origin", "main");
  console.log("setup ok");
}

function status() {
  for (const t of ["runs", "turns", "results", "log"]) {
    console.log(`== ${t}`);
    for (const row of db.prepare(`SELECT * FROM ${t}`).all()) console.log(JSON.stringify(row));
  }
  console.log("== remote branches"); console.log(git(HERE, "ls-remote", "--heads", REMOTE));
  for (const b of git(HERE, "ls-remote", "--heads", REMOTE).split("\n").map((l) => l.split("refs/heads/")[1]).filter((b) => b?.startsWith("task-")))
    console.log(`${b}: ${git(REMOTE, "rev-list", "--count", `main..${b}`)} commit(s)`);
  const left = execFileSync("sh", ["-c", "pgrep -fl 'codex exec' || true"], { encoding: "utf8" }).trim();
  console.log("== codex processes:", left || "none");
}

const [cmd, run, flag] = process.argv.slice(2);
if (cmd === "setup") setup();
else if (cmd === "status") status();
else if (cmd === "run") { await recover(); await runTurn(run, flag?.split("=")[1]); }
else { console.error("usage: setup | run <id> [--crash=before-push|after-push] | status"); process.exit(2); }
