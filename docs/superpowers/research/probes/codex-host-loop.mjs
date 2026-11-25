// Minimal host probe: one vendor (Codex), session takeover across host restarts,
// a host-owned MCP tool (submit_result), and results persisted in SQLite.
// Usage:
//   node host.mjs send <session-name> <text...>   (each run is a fresh host process)
//   node host.mjs dump
import { DatabaseSync } from "node:sqlite";
import { createServer } from "node:http";
import { spawn } from "node:child_process";
import { randomBytes, randomUUID } from "node:crypto";
import { mkdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
// Each probe keeps its state in its own ignored directory so the two never delete each other's files.
const STATE = join(HERE, ".state-codex-host-loop");
mkdirSync(STATE, { recursive: true });
const WORKDIR = join(STATE, "work");
mkdirSync(WORKDIR, { recursive: true });
const db = new DatabaseSync(join(STATE, "host.sqlite"));
db.exec(`
  PRAGMA journal_mode = WAL;
  CREATE TABLE IF NOT EXISTS sessions (
    name TEXT PRIMARY KEY, vendor TEXT NOT NULL, agent_session_id TEXT,
    mcp_token TEXT NOT NULL, created_at TEXT NOT NULL);
  CREATE TABLE IF NOT EXISTS messages (
    id INTEGER PRIMARY KEY, session TEXT NOT NULL, turn INTEGER NOT NULL,
    source TEXT NOT NULL, text TEXT NOT NULL, host_pid INTEGER, at TEXT NOT NULL);
  CREATE TABLE IF NOT EXISTS events (
    id INTEGER PRIMARY KEY, session TEXT NOT NULL, turn INTEGER NOT NULL,
    type TEXT, json TEXT NOT NULL);
  CREATE TABLE IF NOT EXISTS results (
    id INTEGER PRIMARY KEY, session TEXT NOT NULL, turn INTEGER NOT NULL,
    accepted INTEGER NOT NULL, payload TEXT NOT NULL, error TEXT, host_pid INTEGER, at TEXT NOT NULL);
`);
const now = () => new Date().toISOString();

// ---- submit_result schema (validated by the host, not by the agent) ----
const RESULT_SCHEMA = {
  type: "object",
  properties: {
    codeword: { type: "string", description: "Upper-case word, a hyphen, then digits, e.g. ABC-1" },
    turn: { type: "integer" },
  },
  required: ["codeword", "turn"],
  additionalProperties: false,
};
function validate(args) {
  if (!args || typeof args !== "object") return "arguments must be an object";
  const extra = Object.keys(args).filter((k) => !(k in RESULT_SCHEMA.properties));
  if (extra.length) return `unexpected fields: ${extra.join(", ")}`;
  if (typeof args.codeword !== "string" || !/^[A-Z]+-\d+$/.test(args.codeword))
    return "codeword must match ^[A-Z]+-\\d+$ (upper-case letters, hyphen, digits)";
  if (!Number.isInteger(args.turn)) return "turn must be an integer";
  return null;
}

// ---- host MCP server (streamable HTTP, JSON responses only) ----
let currentTurn = 0;
function startMcp() {
  const server = createServer(async (req, res) => {
    if (req.url !== "/mcp") return res.writeHead(404).end();
    if (req.method !== "POST") return res.writeHead(405, { Allow: "POST" }).end();
    const token = (req.headers.authorization || "").replace(/^Bearer /, "");
    const sess = db.prepare("SELECT name FROM sessions WHERE mcp_token = ?").get(token);
    if (!sess) return res.writeHead(401).end();
    let body = "";
    for await (const c of req) body += c;
    const msg = JSON.parse(body);
    const reply = (result) =>
      res.writeHead(200, { "Content-Type": "application/json" }).end(
        JSON.stringify({ jsonrpc: "2.0", id: msg.id, result }));
    if (msg.id === undefined) return res.writeHead(202).end(); // notification
    switch (msg.method) {
      case "initialize":
        return reply({
          protocolVersion: msg.params?.protocolVersion ?? "2025-06-18",
          capabilities: { tools: {} },
          serverInfo: { name: "host-probe", version: "0.0.1" },
        });
      case "ping":
        return reply({});
      case "tools/list":
        return reply({ tools: [{
          name: "submit_result",
          description: "Submit the structured result of this step to the host. The host validates it; if rejected, read the error, fix the arguments and call again.",
          inputSchema: RESULT_SCHEMA,
        }] });
      case "tools/call": {
        const args = msg.params?.arguments;
        const err = msg.params?.name === "submit_result" ? validate(args) : "unknown tool";
        db.prepare(`INSERT INTO results (session, turn, accepted, payload, error, host_pid, at)
                    VALUES (?, ?, ?, ?, ?, ?, ?)`)
          .run(sess.name, currentTurn, err ? 0 : 1, JSON.stringify(args ?? null), err, process.pid, now());
        console.error(`[host] submit_result from ${sess.name}: ${err ? "REJECTED: " + err : "accepted"} ${JSON.stringify(args)}`);
        return reply({
          content: [{ type: "text", text: err ? `Rejected: ${err}` : "Accepted and stored by the host." }],
          isError: !!err,
        });
      }
      default:
        return res.writeHead(200, { "Content-Type": "application/json" }).end(JSON.stringify({
          jsonrpc: "2.0", id: msg.id, error: { code: -32601, message: "method not found" } }));
    }
  });
  return new Promise((r) => server.listen(0, "127.0.0.1", () => r(server)));
}

// ---- Codex runner: one process per turn, resumed by session id ----
function runCodexTurn({ session, text, port }) {
  const common = [
    "--json", "--skip-git-repo-check", "--ignore-user-config",
    "-c", 'sandbox_mode="read-only"',
    "-c", 'approval_policy="never"',
    "-c", `mcp_servers.host.url="http://127.0.0.1:${port}/mcp"`,
    "-c", 'mcp_servers.host.bearer_token_env_var="HOST_MCP_TOKEN"',
    "-c", 'mcp_servers.host.default_tools_approval_mode="approve"',
  ];
  const args = session.agent_session_id
    ? ["exec", "resume", ...common, session.agent_session_id, text]
    : ["exec", ...common, text];
  return new Promise((resolve) => {
    const child = spawn("codex", args, {
      cwd: WORKDIR,
      env: { ...process.env, HOST_MCP_TOKEN: session.mcp_token },
      stdio: ["ignore", "pipe", "pipe"],
    });
    let buf = "", stderr = "", threadId = null, reply = "";
    const tools = [];
    child.stdout.on("data", (d) => {
      buf += d;
      let i;
      while ((i = buf.indexOf("\n")) >= 0) {
        const line = buf.slice(0, i).trim(); buf = buf.slice(i + 1);
        if (!line) continue;
        let ev; try { ev = JSON.parse(line); } catch { continue; }
        db.prepare("INSERT INTO events (session, turn, type, json) VALUES (?, ?, ?, ?)")
          .run(session.name, currentTurn, ev.type ?? null, line);
        if (ev.type === "thread.started") threadId = ev.thread_id;
        const item = ev.item;
        if (ev.type === "item.completed" && item?.type === "agent_message") reply = item.text;
        if (ev.type === "item.completed" && item?.type === "mcp_tool_call") tools.push(item);
      }
    });
    child.stderr.on("data", (d) => (stderr += d));
    child.on("close", (code) => resolve({ code, threadId, reply, tools, stderr }));
  });
}

async function send(name, text) {
  let session = db.prepare("SELECT * FROM sessions WHERE name = ?").get(name);
  if (!session) {
    db.prepare("INSERT INTO sessions (name, vendor, mcp_token, created_at) VALUES (?, 'codex', ?, ?)")
      .run(name, randomBytes(24).toString("hex"), now());
    session = db.prepare("SELECT * FROM sessions WHERE name = ?").get(name);
  }
  currentTurn = (db.prepare("SELECT MAX(turn) AS t FROM messages WHERE session = ?").get(name).t ?? 0) + 1;
  db.prepare("INSERT INTO messages (session, turn, source, text, host_pid, at) VALUES (?, ?, 'user', ?, ?, ?)")
    .run(name, currentTurn, text, process.pid, now());
  const server = await startMcp();
  const port = server.address().port;
  console.error(`[host pid ${process.pid}] turn ${currentTurn}, mcp port ${port}, ` +
    (session.agent_session_id ? `resuming ${session.agent_session_id}` : "new session"));
  const t0 = Date.now();
  const r = await runCodexTurn({ session, text, port });
  server.close();
  if (!session.agent_session_id && r.threadId)
    db.prepare("UPDATE sessions SET agent_session_id = ? WHERE name = ?").run(r.threadId, name);
  db.prepare("INSERT INTO messages (session, turn, source, text, host_pid, at) VALUES (?, ?, 'agent', ?, ?, ?)")
    .run(name, currentTurn, r.reply, process.pid, now());
  console.log(JSON.stringify({
    turn: currentTurn, exit: r.code, seconds: (Date.now() - t0) / 1000,
    threadId: r.threadId, resumedFrom: session.agent_session_id ?? null,
    reply: r.reply,
    toolCalls: r.tools.map((t) => ({ server: t.server, tool: t.tool, status: t.status })),
    stderrTail: r.code ? r.stderr.slice(-800) : undefined,
  }, null, 2));
}

function dump() {
  for (const t of ["sessions", "messages", "results"]) {
    console.log(`== ${t}`);
    for (const row of db.prepare(`SELECT * FROM ${t}`).all()) {
      if (row.mcp_token) row.mcp_token = "<redacted>";
      console.log(JSON.stringify(row));
    }
  }
  console.log("== event types per turn");
  for (const row of db.prepare(
    "SELECT turn, type, COUNT(*) AS n FROM events GROUP BY turn, type ORDER BY turn").all())
    console.log(JSON.stringify(row));
}

const [cmd, name, ...rest] = process.argv.slice(2);
if (cmd === "send") await send(name, rest.join(" "));
else if (cmd === "dump") dump();
else { console.error("usage: send <name> <text> | dump"); process.exit(2); }
