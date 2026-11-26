'use strict'

const state = {
  sessions: [],
  runs: [],
  messagesBySession: new Map(),
  turnsBySession: new Map(),
  runDetailById: new Map(),
  selectedSessionId: null,
  selectedRunId: null,
  showNewSession: false,
  newSessionName: '',
  newSessionCwd: '',
  newSessionError: '',
  showNewTask: false,
  taskInstructions: '',
  taskBase: 'main',
  taskRequestId: null,
  taskError: '',
  unlockOpenFor: new Set(),
  unlockNotes: new Map(),
  unlockErrors: new Map(),
  resolveNotes: new Map(),
  resolveErrors: new Map(),
  pendingDecision: null,
  decisionNote: null,
  confirmMoved: null,
  lockTurns: new Map(),
  lockTurnsByReason: new Map(),
  lockRemoved: null,
  freshDoneIds: new Set(),
  arrivingLocks: new Set(),
  openWorkedTurns: new Set(),
  phoneArea: 'main',
  statusInfo: null,
  hostUnreachable: false,
  composerSending: false,
  composerError: '',
}

function esc(s) {
  return String(s ?? '')
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;')
}

function shortSha(sha) {
  return sha ? sha.slice(0, 7) : ''
}

function fmtTime(iso) {
  if (!iso) return ''
  const d = new Date(iso)
  if (Number.isNaN(d.getTime())) return ''
  return d.toTimeString().slice(0, 5)
}

function fmtDuration(startIso, endIso) {
  if (!startIso) return ''
  const a = Date.parse(startIso)
  const b = endIso ? Date.parse(endIso) : Date.now()
  if (!Number.isFinite(a) || !Number.isFinite(b)) return ''
  const sec = Math.max(0, Math.floor((b - a) / 1000))
  if (sec < 60) return `${sec}s`
  const m = Math.floor(sec / 60)
  const s = sec % 60
  if (m < 60) return `${m}m ${s}s`
  const h = Math.floor(m / 60)
  return `${h}h ${m % 60}m`
}

function initials(name) {
  const parts = String(name || 'S').trim().split(/[\s._-]+/).filter(Boolean)
  if (parts.length >= 2) return (parts[0][0] + parts[1][0]).toUpperCase()
  return String(name || 'S').slice(0, 2).toUpperCase()
}

const GLYPH_LABELS = {
  run: 'running',
  wait: 'waiting',
  need: 'needs you',
  done: 'done',
  int: 'failed',
  lock: 'locked',
}

function glyph(kind, size = 14, fresh = false) {
  const s = size === 10 ? ' style="width:10px;height:10px"' : ''
  const a11y =
    kind === 'idle'
      ? ' role="img" aria-hidden="true"'
      : ` role="img" aria-label="${GLYPH_LABELS[kind] ?? 'idle'}"`
  if (kind === 'run') {
    return `<svg class="g run" viewBox="0 0 14 14"${s}${a11y}><circle class="track" cx="7" cy="7" r="5.5"/><path class="arc" d="M7 1.5a5.5 5.5 0 0 1 5.5 5.5"/></svg>`
  }
  if (kind === 'wait') {
    return `<svg class="g wait" viewBox="0 0 14 14"${s}${a11y}><circle cx="7" cy="7" r="5.5"/></svg>`
  }
  if (kind === 'need') {
    return `<svg class="g need" viewBox="0 0 14 14"${s}${a11y}><circle class="ring" cx="7" cy="7" r="4.5"/><circle class="core" cx="7" cy="7" r="4"/></svg>`
  }
  if (kind === 'done') {
    return `<svg class="g done${fresh ? ' fresh' : ''}" viewBox="0 0 14 14"${s}${a11y}><circle cx="7" cy="7" r="5.5"/><path d="M4.4 7.2l1.8 1.8 3.5-3.6"/></svg>`
  }
  if (kind === 'int') {
    return `<svg class="g int" viewBox="0 0 14 14"${s}${a11y}><circle cx="7" cy="7" r="5.5"/><path d="M5.6 4.8v4.4M8.4 4.8v4.4"/></svg>`
  }
  if (kind === 'lock') {
    return `<svg class="g lockg" viewBox="0 0 14 14"${s}${a11y}><path d="M4.5 6.5V4.8a2.5 2.5 0 0 1 5 0v1.7"/><rect x="2.5" y="6.5" width="9" height="6" rx="1.5"/></svg>`
  }
  return `<svg class="g idle" viewBox="0 0 14 14"${s}${a11y}><circle cx="7" cy="7" r="4.5"/></svg>`
}

function sessionStateWord(s) {
  if (s.frozenReason) return 'locked'
  if (s.busy) return 'running'
  if (s.queuedCount > 0) return 'waiting'
  if (s.lastTurnStatus === 'completed') return 'done'
  if (
    s.lastTurnStatus === 'interrupted' ||
    s.lastTurnStatus === 'cancelled' ||
    s.lastTurnStatus === 'failed'
  ) {
    return 'failed'
  }
  return 'idle'
}

function fmtAgentStart(value) {
  if (!value) return 'unknown'
  const d = new Date(`${value} UTC`)
  if (Number.isNaN(d.getTime())) return value
  const time = d.toTimeString().slice(0, 8)
  const now = new Date()
  const sameDay =
    d.getFullYear() === now.getFullYear() &&
    d.getMonth() === now.getMonth() &&
    d.getDate() === now.getDate()
  if (sameDay) return `started ${time}`
  const months = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec']
  const sameYear = d.getFullYear() === now.getFullYear()
  const date = `${months[d.getMonth()]} ${d.getDate()}${sameYear ? '' : `, ${d.getFullYear()}`}`
  return `started ${date}, ${time}`
}

function endReasonLabel(reason) {
  if (reason === 'exit') return 'finished'
  if (reason === 'cancelled') return 'stopped'
  if (reason === 'host_lost') return 'host stopped'
  if (reason === 'never_started') return 'never started'
  return reason
}

function sessionGlyph(s) {
  if (s.frozenReason) return 'need'
  if (s.busy) return 'run'
  if (s.queuedCount > 0) return 'wait'
  if (s.lastTurnStatus === 'interrupted' || s.lastTurnStatus === 'cancelled' || s.lastTurnStatus === 'failed') return 'int'
  if (s.lastTurnStatus === 'completed') return 'done'
  return 'idle'
}

function runGlyph(r) {
  if (r.status === 'running' || r.status === 'pending') return 'run'
  if (r.status === 'needs_review') return 'need'
  if (r.status === 'done') return 'done'
  if (r.status === 'interrupted' || r.status === 'failed') return 'int'
  return 'idle'
}

function runStatusLabel(status) {
  if (status === 'done') return 'delivered (verified)'
  if (status === 'needs_review') return 'Needs review'
  if (status === 'interrupted') return 'Interrupted'
  if (status === 'running') return 'Running'
  if (status === 'pending') return 'Pending'
  if (status === 'failed') return 'Failed'
  return status
}

function formatInlineText(text) {
  const escaped = esc(text)
  return escaped.replace(/`([^`]+)`/g, (_, code) => `<code class="inl">${code}</code>`)
}

function midPathHtml(path) {
  const s = String(path ?? '')
  const slash = s.lastIndexOf('/')
  let cut = slash > 0 ? slash : s.length > 14 ? s.length - 10 : -1
  if (cut > 0 && s.length - cut > 22) {
    cut = s.length - 16
  }
  if (cut <= 0) {
    return `<span class="mid-path" title="${esc(s)}"><span class="mid-a">${esc(s)}</span></span>`
  }
  const head = s.slice(0, cut)
  const tail = s.slice(cut)
  return `<span class="mid-path" title="${esc(s)}"><span class="mid-a">${esc(head)}</span><span class="mid-b">${esc(tail)}</span></span>`
}

function commitSummaryHtml(detailData, title) {
  if (!detailData) return ''
  const commit = detailData.commit
  if (!commit) {
    return `
      <div class="commit-box">
        <div class="commit-head">${esc(title)}</div>
        <p class="commit-more">The host could not read this commit from the remote.</p>
      </div>
    `
  }
  const files = (commit.files || []).slice(0, 5)
  const filesHtml = files
    .map((f) => {
      const stat =
        f.added === null || f.deleted === null ? 'binary' : `+${f.added} \u2212${f.deleted}`
      return `<div class="commit-file">${midPathHtml(f.path)}<span class="commit-stat">${esc(stat)}</span></div>`
    })
    .join('')
  const more = (commit.filesTotal ?? files.length) - files.length
  const moreHtml =
    more > 0 ? `<p class="commit-more">and ${more} more file${more === 1 ? '' : 's'}</p>` : ''
  return `
    <div class="commit-box">
      <div class="commit-head">${esc(title)}</div>
      <div class="commit-subj">${esc(commit.subject || '(no subject)')}</div>
      ${files.length > 0 ? `<div class="commit-files">${filesHtml}</div>` : ''}
      ${moreHtml}
    </div>
  `
}

function newRequestId() {
  if (globalThis.crypto && typeof globalThis.crypto.randomUUID === 'function') {
    return globalThis.crypto.randomUUID()
  }
  return `req-${Date.now()}-${Math.random().toString(16).slice(2, 10)}`
}

async function api(path, opts) {
  const res = await fetch(path, opts)
  const text = await res.text()
  let data = null
  try {
    data = text ? JSON.parse(text) : null
  } catch {}
  if (!res.ok) {
    const err = new Error(data?.error || `HTTP ${res.status}`)
    err.status = res.status
    err.data = data
    throw err
  }
  return data
}

async function loadLockTurns(sessions) {
  for (const s of sessions) {
    if (!s.frozenReason) {
      state.lockTurns.delete(s.id)
      state.lockTurnsByReason.delete(s.id)
      continue
    }
    if (state.lockTurnsByReason.get(s.id) === s.frozenReason) continue
    try {
      const turns = await api(`/api/sessions/${encodeURIComponent(s.id)}/turns`)
      state.lockTurns.set(s.id, turns)
      state.lockTurnsByReason.set(s.id, s.frozenReason)
    } catch {}
  }
}

async function refreshAll() {
  const [sessions, runs, statusInfo] = await Promise.all([
    api('/api/sessions'),
    api('/api/runs'),
    api('/api/status'),
  ])
  state.sessions = sessions
  state.runs = runs
  state.statusInfo = statusInfo

  if (!state.selectedSessionId || !sessions.some((s) => s.id === state.selectedSessionId)) {
    state.selectedSessionId = sessions[0]?.id ?? null
  }

  if (state.selectedSessionId) {
    await loadSessionData(state.selectedSessionId)
  }
  await loadLockTurns(sessions)
  if (state.selectedRunId) {
    await loadRunDetail(state.selectedRunId)
  }
  render()
}

async function loadSessionData(sessionId) {
  const [messages, turns] = await Promise.all([
    api(`/api/sessions/${encodeURIComponent(sessionId)}/messages`),
    api(`/api/sessions/${encodeURIComponent(sessionId)}/turns`),
  ])
  state.messagesBySession.set(sessionId, messages)
  state.turnsBySession.set(sessionId, turns)
}

async function loadRunDetail(runId) {
  try {
    const detail = await api(`/api/runs/${encodeURIComponent(runId)}`)
    state.runDetailById.set(runId, detail)
  } catch {}
}

async function openRun(runId, nextPhoneArea) {
  const r = state.runs.find((x) => x.id === runId)
  if (r && r.sessionId !== state.selectedSessionId) {
    state.selectedSessionId = r.sessionId
    state.composerError = ''
  }
  state.selectedRunId = runId
  state.showNewTask = false
  if (nextPhoneArea) state.phoneArea = nextPhoneArea
  await Promise.all([
    loadRunDetail(runId),
    state.selectedSessionId ? loadSessionData(state.selectedSessionId) : Promise.resolve(),
  ])
  const detail = state.runDetailById.get(runId)
  if (detail?.run && detail.run.sessionId !== state.selectedSessionId) {
    state.selectedSessionId = detail.run.sessionId
    state.composerError = ''
    await loadSessionData(state.selectedSessionId)
  }
  render()
}

function attentionItems() {
  const frozen = state.sessions.filter((s) => Boolean(s.frozenReason))
  const reviewRuns = state.runs.filter((r) => r.status === 'needs_review')
  const interruptedRuns = state.runs.filter((r) => r.status === 'interrupted')
  return { frozen, reviewRuns, interruptedRuns }
}

function needySessionIds() {
  const ids = new Set()
  for (const s of state.sessions) {
    if (s.frozenReason) ids.add(s.id)
  }
  for (const r of state.runs) {
    if (r.status === 'needs_review' || r.status === 'interrupted') ids.add(r.sessionId)
  }
  return ids
}

function attentionCount() {
  return needySessionIds().size
}

function renderSidebar() {
  withFocusPreserved(renderSidebarNow)
}

function renderSidebarNow() {
  const count = attentionCount()
  const navBadge = document.getElementById('nav-needs-count')
  const panelBadge = document.getElementById('panel-needs-count')
  const phoneTab = document.getElementById('phone-needs-tab')
  navBadge.textContent = String(count)
  navBadge.hidden = count === 0
  panelBadge.textContent = String(count)
  panelBadge.hidden = count === 0
  phoneTab.innerHTML = count > 0 ? `Needs you <span class="count">${count}</span>` : 'Needs you'

  const navNeeds = document.getElementById('nav-needs')
  navNeeds.setAttribute('aria-current', String(!state.selectedRunId && !state.showNewTask))

  const newBox = document.getElementById('new-session-box')
  if (state.showNewSession) {
    newBox.innerHTML = `
      <form class="new-session" id="new-session-form">
        <input class="field" id="new-session-name" placeholder="Session name" value="${esc(state.newSessionName)}" required>
        <input class="field mono" id="new-session-cwd" placeholder="/path/to/git/worktree" value="${esc(state.newSessionCwd)}" required>
        ${state.newSessionError ? `<p class="err-line">${esc(state.newSessionError)}</p>` : ''}
        <div class="form-row">
          <button class="btn quiet" type="button" id="cancel-new-session">Cancel</button>
          <button class="btn" type="submit">Create</button>
        </div>
      </form>
    `
    document.getElementById('new-session-name').addEventListener('input', (e) => {
      state.newSessionName = e.target.value
    })
    document.getElementById('new-session-cwd').addEventListener('input', (e) => {
      state.newSessionCwd = e.target.value
    })
    document.getElementById('cancel-new-session').addEventListener('click', () => {
      state.showNewSession = false
      state.newSessionError = ''
      renderSidebar()
    })
    document.getElementById('new-session-form').addEventListener('submit', async (e) => {
      e.preventDefault()
      state.newSessionError = ''
      try {
        const created = await api('/api/sessions', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ name: state.newSessionName, cwd: state.newSessionCwd }),
        })
        state.showNewSession = false
        state.newSessionName = ''
        state.newSessionCwd = ''
        state.selectedSessionId = created.id
        state.selectedRunId = null
        await refreshAll()
      } catch (err) {
        state.newSessionError = err.message
        renderSidebar()
      }
    })
  } else {
    newBox.innerHTML = ''
  }

  const list = document.getElementById('sessions-list')
  if (state.sessions.length === 0) {
    list.innerHTML = `<p class="empty" style="padding:8px 10px">No sessions yet. Click + to add a git work tree.</p>`
    return
  }

  let html = ''
  for (const s of state.sessions) {
    const selected = s.id === state.selectedSessionId
    const gKind = sessionGlyph(s)
    const statusParts = []
    if (s.busy) statusParts.push('<span class="sbadge running">running</span>')
    if (s.queuedCount > 0) statusParts.push(`<span class="sbadge queued">${s.queuedCount} queued</span>`)
    if (s.frozenReason) statusParts.push('<span class="sbadge frozen">frozen</span>')
    const metaText = s.frozenReason
      ? `Locked · ${s.frozenReason}`
      : s.busy
        ? 'Running'
        : s.queuedCount > 0
          ? `${s.queuedCount} queued`
          : s.lastTurnStatus || 'Idle'
    const rightEl = s.frozenReason
      ? glyph('lock')
      : statusParts.length > 0
        ? statusParts[0]
        : ''

    html += `
      <button class="sess" type="button" data-session-id="${esc(s.id)}" aria-current="${selected}">
        <span class="avatar">${esc(initials(s.name))}<span class="dot">${glyph(gKind, 10)}</span></span>
        <span class="name" title="${esc(s.name)}">${esc(s.name)}</span>
        <span class="right">${rightEl}</span>
        <span class="meta" title="${esc(metaText)}">${esc(metaText)}</span>
      </button>
    `
    const sRuns = state.runs.filter((r) => r.sessionId === s.id)
    for (const r of sRuns) {
      const rg = runGlyph(r)
      const fresh = state.freshDoneIds.has(`run:${r.id}`)
      html += `
        <button class="child" type="button" data-run-id="${esc(r.id)}" aria-current="${state.selectedRunId === r.id}">
          ${glyph(rg, 14, fresh)}
          <span title="${esc(r.instructions)}">${esc(r.instructions)}</span>
          <time>${esc(fmtTime(r.updatedAt))}</time>
        </button>
      `
    }
  }
  list.innerHTML = html

  for (const btn of list.querySelectorAll('[data-session-id]')) {
    btn.addEventListener('click', async () => {
      if (btn.dataset.sessionId !== state.selectedSessionId) {
        state.composerError = ''
      }
      state.selectedSessionId = btn.dataset.sessionId
      state.lockRemoved = null
      state.phoneArea = 'main'
      const shown = state.runs.find((r) => r.id === state.selectedRunId)
      if (shown && shown.sessionId !== state.selectedSessionId) {
        state.selectedRunId = null
        state.pendingDecision = null
      }
      await loadSessionData(state.selectedSessionId)
      render()
    })
  }
  for (const btn of list.querySelectorAll('[data-run-id]')) {
    btn.addEventListener('click', async () => {
      await openRun(btn.dataset.runId, 'side')
    })
  }
}

function stepHtml(ev, turnStatus) {
  if (ev.kind === 'tool.call') {
    const isShell = ev.name === 'shell'
    const icon = isShell ? '#i-term' : '#i-file'
    const command = ev.input?.command ?? ev.name ?? ''
    const label = isShell
      ? `<code title="${esc(command)}">${esc(command)}</code>`
      : `Tool <code title="${esc(ev.name)}">${esc(ev.name)}</code>`
    const g = turnStatus === 'running' ? glyph('run') : glyph('done')
    return `<div class="step">${g}<svg class="i"><use href="${icon}"/></svg>${label}</div>`
  }
  if (ev.kind === 'tool.result') {
    const icon = ev.name === 'shell' ? '#i-term' : '#i-file'
    const g = ev.ok ? glyph('done') : glyph('int')
    return `<div class="step">${g}<svg class="i"><use href="${icon}"/></svg><span title="${esc(ev.name)}">${esc(ev.name)} ${ev.ok ? 'completed' : 'failed'}</span></div>`
  }
  if (ev.kind === 'error') {
    return `<div class="step">${glyph('int')}<svg class="i"><use href="#i-term"/></svg><span title="${esc(ev.message)}">${esc(ev.message)}</span></div>`
  }
  return ''
}

function renderStream() {
  const tabbar = document.getElementById('session-tabbar')
  let tabHtml = ''
  for (const s of state.sessions) {
    const active = s.id === state.selectedSessionId
    const gKind = s.frozenReason ? 'lock' : sessionGlyph(s)
    tabHtml += `<button class="tab" type="button" role="tab" data-tab-session="${esc(s.id)}" aria-selected="${active}" aria-label="${esc(s.name)}, ${esc(sessionStateWord(s))}">${glyph(gKind)}<span title="${esc(s.name)}">${esc(s.name)}</span></button>`
  }
  tabHtml += `<button class="iconbtn" id="tab-new-session" type="button" aria-label="Open session" title="New session"><svg class="i"><use href="#i-plus"/></svg></button>`
  tabbar.innerHTML = tabHtml

  for (const btn of tabbar.querySelectorAll('[data-tab-session]')) {
    btn.addEventListener('click', async () => {
      if (btn.dataset.tabSession !== state.selectedSessionId) {
        state.composerError = ''
      }
      state.selectedSessionId = btn.dataset.tabSession
      state.lockRemoved = null
      const shown = state.runs.find((r) => r.id === state.selectedRunId)
      if (shown && shown.sessionId !== state.selectedSessionId) {
        state.selectedRunId = null
        state.pendingDecision = null
      }
      await loadSessionData(state.selectedSessionId)
      render()
    })
  }
  document.getElementById('tab-new-session')?.addEventListener('click', () => {
    state.showNewSession = true
    state.phoneArea = 'rail'
    render()
  })

  const log = document.getElementById('stream-log')
  const s = state.sessions.find((x) => x.id === state.selectedSessionId)
  if (!s) {
    log.innerHTML = `<div class="sys"><svg class="i"><use href="#i-play"/></svg>No session selected. Create a session to begin.</div>`
    updateComposer(null)
    return
  }

  const messages = state.messagesBySession.get(s.id) ?? []
  const turns = state.turnsBySession.get(s.id) ?? []
  const turnById = new Map(turns.map((t) => [t.id, t]))
  const sRuns = state.runs.filter((r) => r.sessionId === s.id)
  const runById = new Map(sRuns.map((r) => [r.id, r]))

  let html = ''
  if (s.agentSessionId) {
    html += `<div class="sys"><svg class="i"><use href="#i-play"/></svg>Session thread <span class="mono">${esc(s.agentSessionId.slice(0, 12))}</span> · <span class="mono">${esc(fmtTime(s.createdAt))}</span></div>`
  }

  const renderedTurns = new Set()
  const renderWorkedFor = (turn) => {
    if (!turn || renderedTurns.has(turn.id)) return ''
    renderedTurns.add(turn.id)
    const steps = (turn.events || []).map((ev) => stepHtml(ev, turn.status)).filter(Boolean)
    if (steps.length === 0 && turn.status !== 'running') return ''
    const dur = fmtDuration(turn.startedAt, turn.endedAt)
    const summaryLabel = turn.status === 'running' ? `Working for ${dur || '0s'}` : `Worked for ${dur || '0s'}`
    const isOpen = state.openWorkedTurns.has(turn.id) || turn.status === 'running'
    return `
      <details class="worked" data-worked-turn="${turn.id}" ${isOpen ? 'open' : ''}>
        <summary><span>${esc(summaryLabel)}</span><svg class="i chev"><use href="#i-chev"/></svg></summary>
        ${steps.length > 0 ? `<div class="steps">${steps.join('')}</div>` : '<div class="steps"><div class="step">' + glyph('run') + '<span>Waiting for agent events…</span></div></div>'}
      </details>
    `
  }

  for (const m of messages) {
    if (m.source === 'user') {
      if (m.queued) {
        html += `<div class="queued"><div class="who">${glyph('wait')}Waiting · sent ${esc(fmtTime(m.at))}</div>${esc(m.text)}</div>`
      } else {
        const turnTag = m.turnId !== null ? `<span class="turn-tag mono">Turn #${m.turnId}</span>` : ''
        html += `<div class="msg user-wrap"><div class="msg user"><p>${esc(m.text)}</p></div>${turnTag}</div>`
        if (m.turnId !== null && turnById.has(m.turnId)) {
          html += renderWorkedFor(turnById.get(m.turnId))
        }
      }
    } else if (m.source === 'agent') {
      if (m.turnId !== null && turnById.has(m.turnId)) {
        html += renderWorkedFor(turnById.get(m.turnId))
      }
      html += `<div class="msg"><p>${formatInlineText(m.text)}</p></div>`
    } else {
      html += `<div class="sys"><svg class="i"><use href="#i-play"/></svg><span>${esc(m.text)}</span> <span class="mono">${esc(fmtTime(m.at))}</span></div>`
    }
  }

  // Render any run turns or unbound turns with events not yet shown
  for (const t of turns) {
    if (t.runId && runById.has(t.runId)) {
      const r = runById.get(t.runId)
      const rg = runGlyph(r)
      const fresh = state.freshDoneIds.has(`run:${r.id}`)
      html += `<div class="act"><svg class="i"><use href="#i-link"/></svg>Task <button class="act-btn" type="button" data-open-run="${esc(r.id)}"><span class="chip" title="${esc(r.instructions)}">${glyph(rg, 14, fresh)}<span class="mono">${esc(r.id)}</span>${esc(r.instructions)}</span></button></div>`
    }
    html += renderWorkedFor(t)
  }

  if (s.frozenReason) {
    html += `<div class="sys lock-line"><span class="lock-line-main">${glyph('lock')}<span class="lock-line-text">Locked: ${esc(s.frozenReason)}. Messages wait until you remove the lock.</span></span><button class="btn quiet" type="button" data-stream-unlock="${esc(s.id)}">Remove lock…</button></div>`
  }

  log.innerHTML = html

  for (const det of log.querySelectorAll('details[data-worked-turn]')) {
    det.addEventListener('toggle', () => {
      const id = Number(det.dataset.workedTurn)
      if (det.open) state.openWorkedTurns.add(id)
      else state.openWorkedTurns.delete(id)
    })
  }
  for (const btn of log.querySelectorAll('[data-stream-unlock]')) {
    btn.addEventListener('click', () => {
      const sid = btn.dataset.streamUnlock
      state.selectedRunId = null
      state.pendingDecision = null
      state.unlockOpenFor.add(sid)
      state.lockRemoved = null
      state.phoneArea = 'side'
      render()
      const field = document.querySelector(`[data-unlock-form="${CSS.escape(sid)}"] textarea`)
      if (field) field.focus()
    })
  }
  for (const btn of log.querySelectorAll('[data-open-run]')) {
    btn.addEventListener('click', async () => {
      await openRun(btn.dataset.openRun, 'side')
    })
  }

  updateComposer(s)
}

function renderComposerError() {
  const errEl = document.getElementById('composer-error')
  if (!errEl) return
  errEl.textContent = state.composerError
  errEl.hidden = !state.composerError
}

function updateComposer(session) {
  const input = document.getElementById('composer-input')
  const sendBtn = document.getElementById('send-btn')
  const cancelBtn = document.getElementById('cancel-btn')
  const under = document.getElementById('composer-under')

  if (!session) {
    input.disabled = true
    sendBtn.disabled = true
    cancelBtn.hidden = true
    input.placeholder = 'Create a session to start chatting'
    under.innerHTML = ''
    renderComposerError()
    return
  }

  input.disabled = false
  sendBtn.disabled = Boolean(state.composerSending)
  input.placeholder = `Ask ${session.name} — messages sent during a turn wait their turn`
  cancelBtn.hidden = !session.busy
  under.innerHTML = `<span>This Mac</span><span class="mono" title="${esc(session.cwd)}">${esc(session.cwd)}</span>`
  renderComposerError()
}

function lockTurnOf(session) {
  const turns = state.lockTurns.get(session.id) ?? []
  const named = /turn (\d+)/.exec(session.frozenReason || '')
  const wanted = named ? Number(named[1]) : null
  const namedTurn = wanted === null ? null : turns.find((t) => t.id === wanted)
  return namedTurn ?? turns[turns.length - 1] ?? null
}

function lockEvidenceHtml(session) {
  const turn = lockTurnOf(session)
  if (!turn) return ''
  const run = turn.runId ? state.runs.find((r) => r.id === turn.runId) : null
  const path = run ? run.worktreePath : session.cwd
  const hasPgid = turn.agentPgid !== null && turn.agentPgid !== undefined
  const agentText = hasPgid
    ? `process group ${turn.agentPgid}, ${fmtAgentStart(turn.agentStartedAt)}`
    : 'never started'
  const endedText = `ended ${fmtTime(turn.endedAt)}${turn.endReason ? ` (${endReasonLabel(turn.endReason)})` : ''}`
  const psCode = hasPgid ? `<code class="inl">ps -g ${esc(String(turn.agentPgid))}</code> shows nothing` : ''
  return `
      <dl class="lock-facts">
        <dt>Agent process</dt><dd>${esc(agentText)}</dd>
        <dt>Turn</dt><dd>Turn ${esc(String(turn.id))}, ${esc(endedText)}</dd>
        <dt>Working directory</dt><dd class="mono one-line" title="${esc(path)}">${esc(path)}</dd>
      </dl>
      <p class="lock-check">To check on the Mac: ${psCode ? `${psCode}, and ` : ''}<code class="inl">git -C ${esc(path)} status</code> shows no change you did not expect.</p>
  `
}

function lockConsequenceHtml(session) {
  const waiting = state.runs.filter((r) => r.sessionId === session.id && r.status === 'needs_review')
  const lines = [`Removing the lock lets ${esc(session.name)} run again.`]
  for (const r of waiting) {
    lines.push(`Run <span class="mono">${esc(r.id)}</span> will then wait for your decision.`)
  }
  if (session.queuedCount > 0) {
    lines.push(`${session.queuedCount} queued message${session.queuedCount === 1 ? '' : 's'} will be sent.`)
  }
  return `<div class="unlock-what">${lines.map((line) => `<p>${line}</p>`).join('')}</div>`
}

function lockRemovedHtml(session) {
  if (!state.lockRemoved || state.lockRemoved.sessionId !== session.id) return ''
  const runs = state.lockRemoved.runIds ?? []
  const tail = runs
    .map((id) => `Run <span class="mono">${esc(id)}</span> now waits for your decision.`)
    .join(' ')
  return `<p class="lock-removed">${glyph('done')}<span>Lock removed on ${esc(session.name)}.${tail ? ` ${tail}` : ''}</span></p>`
}

function lockCardHtml(session) {
  const isOpen = state.unlockOpenFor.has(session.id)
  const note = state.unlockNotes.get(session.id) ?? ''
  const err = state.unlockErrors.get(session.id) ?? ''
  const arrive = state.arrivingLocks.has(session.id) ? ' arrive' : ''
  return `
    <article class="lock${isOpen ? ' open' : ''}${arrive}" data-lock-session="${esc(session.id)}">
      <div class="top">${glyph('lock')}${esc(session.name)} locked<time>${esc(fmtTime(session.frozenAt))}</time></div>
      <p>${esc(session.frozenReason || 'Session is frozen')}. Nothing runs in ${esc(session.name)} until you remove the lock.</p>
      ${lockEvidenceHtml(session)}
      <div><button class="btn warn open-btn" type="button" data-open-unlock="${esc(session.id)}">Remove lock…</button></div>
      <form class="unlock" data-unlock-form="${esc(session.id)}">
        <textarea id="unlock-note-${esc(session.id)}" placeholder="Why it is safe now, e.g. no git or test processes left" aria-label="Why it is safe now">${esc(note)}</textarea>
        ${lockConsequenceHtml(session)}
        ${err ? `<p class="err-line">${esc(err)}</p>` : ''}
        <div class="row">
          <button class="btn quiet" type="button" data-keep-locked="${esc(session.id)}">Keep locked</button>
          <button class="btn warn" type="submit">Remove lock with this note</button>
        </div>
      </form>
    </article>
  `
}

function bindLockHandlers(container) {
  for (const btn of container.querySelectorAll('[data-open-unlock]')) {
    btn.addEventListener('click', () => {
      const sid = btn.dataset.openUnlock
      state.unlockOpenFor.add(sid)
      renderSide()
      const field = document.querySelector(`[data-unlock-form="${sid}"] textarea`)
      if (field) field.focus()
    })
  }
  for (const btn of container.querySelectorAll('[data-keep-locked]')) {
    btn.addEventListener('click', () => {
      const sid = btn.dataset.keepLocked
      state.unlockOpenFor.delete(sid)
      state.unlockErrors.delete(sid)
      renderSide()
      const trigger = document.querySelector(`[data-open-unlock="${sid}"]`)
      if (trigger) trigger.focus()
    })
  }
  for (const form of container.querySelectorAll('[data-unlock-form]')) {
    const sid = form.dataset.unlockForm
    const ta = form.querySelector('textarea')
    ta.addEventListener('input', () => {
      state.unlockNotes.set(sid, ta.value)
    })
    ta.addEventListener('keydown', (e) => {
      if (e.key !== 'Escape') return
      e.preventDefault()
      state.unlockOpenFor.delete(sid)
      state.unlockErrors.delete(sid)
      renderSide()
      const trigger = document.querySelector(`[data-open-unlock="${sid}"]`)
      if (trigger) trigger.focus()
    })
    form.addEventListener('submit', async (e) => {
      e.preventDefault()
      const note = (state.unlockNotes.get(sid) ?? '').trim()
      if (!note) {
        state.unlockErrors.set(sid, 'Enter a note explaining why it is safe to unfreeze.')
        renderSide()
        return
      }
      state.unlockErrors.delete(sid)
      const waiting = state.runs
        .filter((r) => r.sessionId === sid && r.status === 'needs_review')
        .map((r) => r.id)
      try {
        await api(`/api/sessions/${encodeURIComponent(sid)}/unfreeze`, {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ note }),
        })
        state.unlockOpenFor.delete(sid)
        state.unlockNotes.delete(sid)
        state.lockRemoved = { sessionId: sid, runIds: waiting }
        await refreshAll()
      } catch (err) {
        state.unlockErrors.set(sid, err.message)
        renderSide()
      }
    })
  }
}

function newTaskHtml() {
  const s = state.sessions.find((x) => x.id === state.selectedSessionId)
  if (!s) {
    return `<section class="new-task-wrap" id="new-task-section"><p class="empty">Create or select a session before starting a task.</p></section>`
  }
  const target = s.target || 'no origin remote found'
  return `
    <section class="new-task-wrap" id="new-task-section">
      <h2 class="title">New task</h2>
      <p class="session-line">Session: <b>${esc(s.name)}</b></p>
      <form class="new-task" id="new-task-form">
        <label>Target (read-only)
          <input class="field mono" value="${esc(target)}" readonly>
        </label>
        <label for="task-base">Base branch or commit
          <input class="field mono" id="task-base" value="${esc(state.taskBase)}" placeholder="main">
        </label>
        <label for="task-instructions">Instructions
          <textarea class="field" id="task-instructions" rows="3" placeholder="Describe the change for the worktree task">${esc(state.taskInstructions)}</textarea>
        </label>
        ${state.taskError ? `<p class="err-line">${esc(state.taskError)}</p>` : ''}
        <div class="form-row">
          <button class="btn quiet" type="button" id="cancel-new-task">Cancel</button>
          <button class="btn" type="submit">Start task</button>
        </div>
      </form>
    </section>
  `
}

function bindNewTask(container) {
  const s = state.sessions.find((x) => x.id === state.selectedSessionId)
  if (!s) return
  const cancel = document.getElementById('cancel-new-task')
  if (cancel) {
    cancel.addEventListener('click', () => {
      state.showNewTask = false
      state.taskError = ''
      renderSide()
      const toggle = document.getElementById('new-task-toggle')
      if (toggle) toggle.focus()
    })
  }
  document.getElementById('task-base').addEventListener('input', (e) => {
    state.taskBase = e.target.value
    state.taskRequestId = null
  })
  document.getElementById('task-instructions').addEventListener('input', (e) => {
    state.taskInstructions = e.target.value
    state.taskRequestId = null
  })
  document.getElementById('new-task-form').addEventListener('submit', async (e) => {
    e.preventDefault()
    state.taskError = ''
    if (!state.taskInstructions.trim()) {
      state.taskError = 'Instructions are required.'
      renderSide()
      const field = document.getElementById('task-instructions')
      if (field) field.focus()
      return
    }
    if (!state.taskRequestId) {
      state.taskRequestId = newRequestId()
    }
    try {
      const created = await api('/api/runs', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          sessionId: s.id,
          requestId: state.taskRequestId,
          instructions: state.taskInstructions,
          base: state.taskBase.trim() || 'main',
        }),
      })
      state.taskRequestId = null
      state.taskInstructions = ''
      state.showNewTask = false
      state.selectedRunId = created.id
      await refreshAll()
    } catch (err) {
      state.taskError = err.message
      renderSide()
    }
  })
}

function renderRunDetail(container, runId) {
  const run = state.runs.find((r) => r.id === runId)
  if (!run) {
    state.selectedRunId = null
    renderSide()
    return
  }
  const runSession = state.sessions.find((s) => s.id === run.sessionId)
  if (state.lockRemoved && state.lockRemoved.sessionId === run.sessionId && runSession?.frozenReason) {
    state.lockRemoved = null
  }
  if (state.decisionNote && state.decisionNote.runId !== runId) state.decisionNote = null
  if (state.pendingDecision && state.pendingDecision.runId !== runId) state.pendingDecision = null
  if (state.confirmMoved && state.confirmMoved.runId !== runId) state.confirmMoved = null
  let focusAcceptAfterRender = false
  if (
    state.pendingDecision &&
    state.pendingDecision.runId === run.id &&
    (state.pendingDecision.sha ?? null) !== (run.observedSha ?? null)
  ) {
    state.pendingDecision = null
    state.confirmMoved = {
      runId: run.id,
      text: `The branch tip moved to ${shortSha(run.observedSha)} while you were confirming. Review it, then accept again.`,
    }
    focusAcceptAfterRender = true
  }

  const detailData = state.runDetailById.get(runId)
  const turns = detailData?.turns ?? []
  const session = state.sessions.find((s) => s.id === run.sessionId)
  const frozen = Boolean(session?.frozenReason)
  const lockNotice = frozen
    ? `<p class="ask-note">Remove the lock on <b>${esc(session.name)}</b> first. The lock means an agent process may still be running; check that none is, then remove it.</p>`
    : ''
  const rg = runGlyph(run)
  const fresh = state.freshDoneIds.has(`run:${run.id}`)
  const tipSha = run.resultSha || run.observedSha
  const pending = state.pendingDecision?.runId === run.id ? state.pendingDecision.decision : null
  const decisionDone =
    state.decisionNote && state.decisionNote.runId === run.id
      ? `<p class="decision-done">${glyph('done')}<span>${esc(state.decisionNote.text)}</span></p>`
      : ''
  const confirmMovedLine =
    state.confirmMoved && state.confirmMoved.runId === run.id
      ? `<p class="confirm-moved">${glyph('need')}<span>${esc(state.confirmMoved.text)}</span></p>`
      : ''

  let tlHtml = `<div class="tl"><svg class="i"><use href="#i-plus"/></svg><span><b>Run created</b> on base <span class="mono">${esc(shortSha(run.baseSha))}</span></span><time>${esc(fmtTime(run.updatedAt))}</time></div>`
  for (const t of turns) {
    tlHtml += `<div class="tl"><svg class="i"><use href="#i-play"/></svg><span>Turn <span class="mono">#${t.id}</span> · ${esc(t.status)}${t.endReason ? ` (${esc(endReasonLabel(t.endReason))})` : ''}</span><time>${esc(fmtTime(t.endedAt || t.startedAt))}</time></div>`
    if (t.writerCheck === 'possible') {
      tlHtml += `<div class="tl">${glyph('lock')}<span>Writer check <b>possible</b>, session locked</span><time>${esc(fmtTime(t.endedAt))}</time></div>`
    }
  }
  if (run.resolution) {
    tlHtml += `<div class="tl"><svg class="i"><use href="#i-commit"/></svg><span>Resolved: <span class="mono">${esc(run.resolution)}</span></span><time>${esc(fmtTime(run.updatedAt))}</time></div>`
  }

  let askHtml = ''
  if (run.status === 'needs_review') {
    const note = state.resolveNotes.get(run.id) ?? ''
    const err = state.resolveErrors.get(run.id) ?? ''
    const obs7 = shortSha(run.observedSha)
    const refuse = ''
    void refuse
    const acceptOff = run.observedSha ? '' : ' disabled'
    const retryOff = run.observedSha ? ' disabled' : ''
    const pills = `<div class="opts">
          <button class="pill go" type="button" data-decide="accept"${acceptOff}>Accept ${obs7 ? `<span class="mono">${esc(obs7)}</span>` : ''}</button>
          <button class="pill" type="button" data-decide="fail">Mark failed</button>
          <button class="pill" type="button" data-decide="allow_retry"${retryOff}>Allow retry</button>
        </div>
        <p class="fine">${
          run.observedSha
            ? `Accept applies to ${esc(obs7)} only (${esc(run.observedSha)}). If the branch moves first, you will be asked to review the new tip.`
            : 'No branch sha is currently observed on the remote.'
        }</p>
        <p class="fine reason">${
          run.observedSha
            ? 'Allow retry is disabled: the branch is on the remote.'
            : 'Accept is disabled: there is no observed sha to accept.'
        }</p>`
    const pendingSha7 = shortSha(state.pendingDecision?.sha)
    const confirmLabel =
      pending === 'accept'
        ? `Confirm accept ${pendingSha7 ? `<span class="mono">${esc(pendingSha7)}</span>` : ''}`
        : pending === 'fail'
          ? 'Confirm fail'
          : 'Confirm retry'
    const confirmRow = `<div class="opts confirm">
          <button class="pill${pending === 'accept' ? ' go' : ''}" type="button" id="confirm-decision">${confirmLabel}</button>
          <button class="pill quiet" type="button" id="back-from-confirm">Back</button>
        </div>`
    const questionHtml = frozen
      ? `<span>${esc(run.detail || 'This run needs review.')}</span>`
      : `<span>${
          obs7
            ? `Review <span class="mono">${esc(obs7)}</span>: accept it, mark it failed, or allow a retry.`
            : 'Review this run: mark it failed or allow a retry.'
        }</span>`
    const trimmedDetail = (run.detail || '').trim()
    const capDetail = trimmedDetail ? trimmedDetail.charAt(0).toUpperCase() + trimmedDetail.slice(1) : ''
    const hostNoteHtml =
      !frozen && capDetail
        ? `<p class="host-note">Host note: ${esc(capDetail.endsWith('.') ? capDetail : `${capDetail}.`)}</p>`
        : ''
    askHtml = `
      <section class="ask" id="host-question">
        <div class="who"><span class="avatar sm" style="background:var(--silver-metal);color:#15161a;box-shadow:none"><svg class="i" style="width:11px;height:11px;stroke-width:2"><use href="#i-mark"/></svg></span><b>Host</b><time>${esc(fmtTime(run.updatedAt))}</time></div>
        <div class="q">${glyph('need')}${questionHtml}</div>
        ${hostNoteHtml}
        ${commitSummaryHtml(detailData, 'What changed')}
        ${
          frozen
            ? lockNotice
            : `<label class="field-label" for="resolve-note">Note</label>
        <textarea class="note-field" id="resolve-note" placeholder="Required for Mark failed and Allow retry">${esc(note)}</textarea>
        ${err ? `<p class="err-line">${esc(err)}</p>` : ''}
        ${pending ? confirmRow : pills}`
        }
      </section>
    `
  } else if (run.status === 'interrupted') {
    const err = state.resolveErrors.get(run.id) ?? ''
    askHtml = `
      <section class="ask" id="host-question">
        <div class="who"><span class="avatar sm" style="background:var(--silver-metal);color:#15161a;box-shadow:none"><svg class="i" style="width:11px;height:11px;stroke-width:2"><use href="#i-mark"/></svg></span><b>Host</b><time>${esc(fmtTime(run.updatedAt))}</time></div>
        <div class="q">${glyph('int')}<span>${esc(run.detail || 'Run interrupted; safe to retry.')}</span></div>
        ${err ? `<p class="err-line">${esc(err)}</p>` : ''}
        ${
          frozen
            ? lockNotice
            : `<div class="opts"><button class="pill" type="button" id="retry-run-btn">Retry (${run.retries} of 3 used)</button></div>`
        }
      </section>
    `
  }

  container.innerHTML = `
    <button class="crumb" type="button" id="back-to-needs"><svg class="i"><use href="#i-back"/></svg><span>Viewing</span><span class="avatar sm">${esc(initials(session?.name))}</span><span>${esc(session?.name || run.sessionId)}</span><span class="dim">·</span><span class="mono">${esc(run.id)}</span></button>
    <h1 class="title"><span class="mono">${esc(run.id)}</span>${esc(run.instructions)}</h1>
    <div class="statusrow">
      <span class="statuslabel">${glyph(rg, 14, fresh)}${esc(runStatusLabel(run.status))}</span>
      <span>${esc(fmtTime(run.updatedAt))}</span>
    </div>
    ${confirmMovedLine}
    ${
      run.status === 'done'
        ? `${commitSummaryHtml(detailData, 'Accepted commit')}<p class="empty">The branch matches the accepted result. Whether the work meets the request is judged by tests or review.</p>`
        : ''
    }
    <div class="chips">
      <span class="chip" title="${esc(run.branch)}"><svg class="i" style="width:14px;height:14px"><use href="#i-branch"/></svg><span class="mono">${esc(run.branch)}</span></span>
      ${tipSha ? `<span class="chip" title="${esc(tipSha)}"><svg class="i" style="width:14px;height:14px"><use href="#i-commit"/></svg>tip <span class="mono">${esc(shortSha(tipSha))}</span></span>` : ''}
      <span class="chip" title="${esc(run.baseSha)}"><svg class="i" style="width:14px;height:14px"><use href="#i-commit"/></svg>base <span class="mono">${esc(shortSha(run.baseSha))}</span></span>
    </div>
    ${decisionDone}
    ${session?.frozenReason ? lockCardHtml(session) : lockRemovedHtml(session)}
    ${askHtml}
    <dl class="kv">
      <dt>Session</dt><dd>${esc(session?.name || run.sessionId)}</dd>
      <dt>Target</dt><dd class="mono one-line" title="${esc(run.remote)}">${esc(run.remote)}</dd>
      <dt>Retries</dt><dd>${run.retries} of 3</dd>
      ${tipSha ? `<dt>SHA</dt><dd class="mono">${esc(tipSha)}</dd>` : ''}
      ${run.detail ? `<dt>Detail</dt><dd>${esc(run.detail)}</dd>` : ''}
      ${run.resolution ? `<dt>Resolution</dt><dd class="mono">${esc(run.resolution)}</dd>` : ''}
      <dt>Done means</dt><dd>An accepted result whose sha is the branch tip, built on base <span class="mono">${esc(shortSha(run.baseSha))}</span>.</dd>
    </dl>
    <div>
      <h3 class="h4">Activity</h3>
      ${tlHtml}
    </div>
  `

  document.getElementById('back-to-needs').addEventListener('click', () => {
    state.selectedRunId = null
    state.pendingDecision = null
    renderSide()
    renderSidebar()
  })

  bindLockHandlers(container)

  const focusAfter = (selector) => {
    const el = document.querySelector(selector)
    if (el) el.focus()
  }

  const noteEl = document.getElementById('resolve-note')
  if (noteEl) {
    noteEl.addEventListener('input', () => {
      state.resolveNotes.set(run.id, noteEl.value)
    })
  }

  const openConfirm = (decision) => {
    state.resolveErrors.delete(run.id)
    state.confirmMoved = null
    state.pendingDecision = { runId: run.id, decision, sha: run.observedSha ?? null }
    renderSide()
    focusAfter('#confirm-decision')
  }

  const closeConfirm = () => {
    const decision = state.pendingDecision?.decision
    state.pendingDecision = null
    renderSide()
    if (decision) focusAfter(`[data-decide="${decision}"]`)
  }

  const doResolve = async (decision, sha) => {
    state.resolveErrors.delete(run.id)
    const note = (state.resolveNotes.get(run.id) ?? '').trim()
    if (decision !== 'accept' && !note) {
      state.resolveErrors.set(
        run.id,
        decision === 'fail'
          ? 'Enter a note before marking this run failed.'
          : 'Enter a note before allowing a retry.',
      )
      renderSide()
      focusAfter('#resolve-note')
      return
    }
    try {
      await api(`/api/runs/${encodeURIComponent(run.id)}/resolve`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          decision,
          sha: decision === 'accept' ? sha : undefined,
          note,
        }),
      })
      state.resolveNotes.delete(run.id)
      state.pendingDecision = null
      state.decisionNote = {
        runId: run.id,
        text:
          decision === 'accept'
            ? `Accepted ${shortSha(run.observedSha)}`
            : decision === 'fail'
              ? 'Marked failed'
              : 'Retry allowed',
      }
      await refreshAll()
    } catch (err) {
      state.pendingDecision = null
      if (err.status === 409) {
        await refreshAll()
        const latest = state.runs.find((r) => r.id === run.id)
        const msg =
          decision === 'accept' && latest?.observedSha && latest.observedSha !== run.observedSha
            ? `Branch tip moved to ${shortSha(latest.observedSha)} (${latest.observedSha}). Review the new sha before accepting.`
            : err.message
        state.resolveErrors.set(run.id, msg)
        renderSide()
      } else {
        state.resolveErrors.set(run.id, err.message)
        renderSide()
      }
    }
  }

  for (const btn of container.querySelectorAll('[data-decide]')) {
    btn.addEventListener('click', () => openConfirm(btn.dataset.decide))
  }
  document.getElementById('confirm-decision')?.addEventListener('click', () => {
    if (state.pendingDecision) {
      void doResolve(state.pendingDecision.decision, state.pendingDecision.sha ?? undefined)
    }
  })
  document.getElementById('back-from-confirm')?.addEventListener('click', closeConfirm)

  if (focusAcceptAfterRender) focusAfter('[data-decide="accept"]')

  document.getElementById('retry-run-btn')?.addEventListener('click', async () => {
    state.resolveErrors.delete(run.id)
    try {
      await api(`/api/runs/${encodeURIComponent(run.id)}/retry`, { method: 'POST' })
      await refreshAll()
    } catch (err) {
      state.resolveErrors.set(run.id, err.message)
      renderSide()
    }
  })
}

function renderSide() {
  withFocusPreserved(renderSideNow)
}

function renderSideNow() {
  const body = document.getElementById('side-body')
  if (state.selectedRunId) {
    renderRunDetail(body, state.selectedRunId)
    return
  }

  const { frozen, reviewRuns, interruptedRuns } = attentionItems()
  const groups = []
  for (const session of state.sessions) {
    if (state.lockRemoved && state.lockRemoved.sessionId === session.id && session.frozenReason) {
      state.lockRemoved = null
    }
    const lock = frozen.some((s) => s.id === session.id)
    const reviews = reviewRuns.filter((r) => r.sessionId === session.id)
    const interrupted = interruptedRuns.filter((r) => r.sessionId === session.id)
    if (!lock && reviews.length === 0 && interrupted.length === 0) continue
    groups.push({ session, lock, reviews, interrupted })
  }

  const runCard = (r) => `
        <button class="att-card" type="button" data-pick-run="${esc(r.id)}">
          <div class="top">${glyph(r.status === 'needs_review' ? 'need' : 'int')}<span class="mono">${esc(r.id)}</span><span title="${esc(r.instructions)}">${esc(r.instructions)}</span></div>
          <div class="sub">${esc(r.detail || (r.status === 'needs_review' ? 'Needs review' : 'Safe to retry'))}${r.observedSha ? ` · <span class="mono">${esc(shortSha(r.observedSha))}</span>` : ''}</div>
        </button>
      `

  let html = state.showNewTask ? newTaskHtml() : ''
  if (groups.length > 0) {
    html += `<h3 class="h4">Needs you</h3>`
    for (const g of groups) {
      html += `
        <article class="need-group" data-need-session="${esc(g.session.id)}">
          <div class="need-head">
            <span class="avatar sm">${esc(initials(g.session.name))}</span>
            <b>${esc(g.session.name)}${g.lock ? ' locked' : ''}</b>
            ${g.lock ? glyph('lock') : ''}
          </div>
          ${g.lock ? lockCardHtml(g.session) : lockRemovedHtml(g.session)}
          ${g.reviews.map(runCard).join('')}
          ${g.interrupted.map(runCard).join('')}
        </article>
      `
    }
  }

  const otherRuns = state.runs.filter(
    (r) => r.status !== 'needs_review' && r.status !== 'interrupted',
  )
  if (otherRuns.length > 0) {
    html += `<h3 class="h4">Recent runs</h3>`
    for (const r of otherRuns) {
      const s = state.sessions.find((x) => x.id === r.sessionId)
      const rg = runGlyph(r)
      const fresh = state.freshDoneIds.has(`run:${r.id}`)
      html += `
        <button class="att-card" type="button" data-pick-run="${esc(r.id)}">
          <div class="top">${glyph(rg, 14, fresh)}<span class="mono">${esc(r.id)}</span><span title="${esc(r.instructions)}">${esc(r.instructions)}</span></div>
          <div class="sub">${esc(s?.name || r.sessionId)} · ${esc(runStatusLabel(r.status))}${r.resultSha ? ` · <span class="mono">${esc(shortSha(r.resultSha))}</span>` : ''}</div>
        </button>
      `
    }
  }

  if (!html) {
    html = `<p class="empty">Nothing needs attention right now. Start a task or send a message.</p>`
  }

  body.innerHTML = html
  if (state.showNewTask) bindNewTask(body)
  bindLockHandlers(body)
  for (const btn of body.querySelectorAll('[data-pick-run]')) {
    btn.addEventListener('click', async () => {
      await openRun(btn.dataset.pickRun, null)
    })
  }
}

function renderPhoneBanner() {
  const banner = document.getElementById('phone-banner')
  if (!banner) return
  banner.innerHTML = state.hostUnreachable
    ? `${glyph('need')}<span>Host unreachable · reconnecting…</span>`
    : ''
  banner.hidden = !state.hostUnreachable
}

function phoneStateOf(session) {
  if (!session) return null
  if (session.frozenReason) return { kind: 'lock', label: 'locked' }
  if (session.busy) return { kind: 'run', label: 'running' }
  if (session.queuedCount > 0) return { kind: 'wait', label: 'waiting' }
  return null
}

function renderPhoneHead() {
  const head = document.getElementById('phone-head')
  if (!head) return
  const session = state.sessions.find((s) => s.id === state.selectedSessionId)
  if (!session) {
    head.innerHTML = `<span class="name">No session selected</span>`
    return
  }
  const st = phoneStateOf(session)
  head.innerHTML = `
    <span class="avatar sm">${esc(initials(session.name))}</span>
    <span class="name" title="${esc(session.name)}">${esc(session.name)}</span>
    ${st ? `<span class="state">${glyph(st.kind)}${esc(st.label)}</span>` : ''}
  `
}

function renderStatusBar() {
  const runningCount =
    state.sessions.filter((s) => s.busy).length
  const queuedCount = state.sessions.reduce((acc, s) => acc + (s.queuedCount || 0), 0)
  const needCount = attentionCount()
  const up = state.statusInfo?.startedAt ? fmtDuration(state.statusInfo.startedAt) : '0s'

  const rightSide = state.hostUnreachable
    ? `<span style="color:var(--orange)">${glyph('need')}Host unreachable · reconnecting…</span>`
    : `<span>Dispatch ${state.statusInfo?.dispatchOpen === false ? 'closed' : 'open'} · up <span class="mono">${esc(up)}</span> · codex</span>`

  document.getElementById('statusbar').innerHTML = `
    <span>${glyph(runningCount > 0 ? 'run' : 'idle')}${runningCount} running</span>
    <span>${glyph(queuedCount > 0 ? 'wait' : 'idle')}${queuedCount} queued</span>
    <span>${glyph(needCount > 0 ? 'need' : 'idle')}${needCount} need you</span>
    <span class="grow"></span>
    ${rightSide}
  `
  renderPhoneBanner()
}

function renderPhoneTabs() {
  for (const b of document.querySelectorAll('.tabs button')) {
    b.setAttribute('aria-selected', String(b.dataset.tab === state.phoneArea))
  }
  for (const a of document.querySelectorAll('[data-area]')) {
    a.classList.toggle('shown', a.dataset.area === state.phoneArea)
  }
}

let renderDepth = 0

function focusKeyOf(el) {
  if (el.id) return `id:${el.id}`
  const unlockForm = el.closest('[data-unlock-form]')
  if (unlockForm) return `unlock:${unlockForm.dataset.unlockForm}`
  return null
}

function fieldByFocusKey(key) {
  if (key.startsWith('id:')) return document.getElementById(key.slice(3))
  if (key.startsWith('unlock:')) {
    const sid = key.slice('unlock:'.length)
    return document.querySelector(`[data-unlock-form="${CSS.escape(sid)}"] textarea`)
  }
  return null
}

function captureFieldFocus() {
  const el = document.activeElement
  if (!el || (el.tagName !== 'TEXTAREA' && el.tagName !== 'INPUT')) return null
  if (!el.closest('#side-body, #new-session-box, #composer-form')) return null
  const key = focusKeyOf(el)
  if (!key) return null
  return {
    key,
    start: typeof el.selectionStart === 'number' ? el.selectionStart : null,
    end: typeof el.selectionEnd === 'number' ? el.selectionEnd : null,
    scrollTop: el.scrollTop,
  }
}

function restoreFieldFocus(snap) {
  if (!snap) return
  const el = fieldByFocusKey(snap.key)
  if (!el) return
  if (document.activeElement !== el) el.focus({ preventScroll: true })
  if (snap.start !== null && typeof el.setSelectionRange === 'function') {
    try {
      el.setSelectionRange(snap.start, snap.end ?? snap.start)
    } catch {}
  }
  el.scrollTop = snap.scrollTop
}

function withFocusPreserved(fn) {
  if (renderDepth > 0) {
    fn()
    return
  }
  renderDepth++
  const snap = captureFieldFocus()
  try {
    fn()
  } finally {
    renderDepth--
    restoreFieldFocus(snap)
  }
}

function render() {
  withFocusPreserved(renderNow)
}

function renderNow() {
  renderPhoneTabs()
  renderPhoneBanner()
  renderPhoneHead()
  renderSidebar()
  renderStream()
  renderSide()
  renderStatusBar()
}

document.addEventListener('keydown', (e) => {
  if (e.key !== 'Escape' || !state.pendingDecision) return
  const decision = state.pendingDecision.decision
  state.pendingDecision = null
  renderSide()
  const pill = document.querySelector(`[data-decide="${decision}"]`)
  if (pill) pill.focus()
})

document.getElementById('new-session-btn').addEventListener('click', () => {
  state.showNewSession = !state.showNewSession
  state.newSessionError = ''
  renderSidebar()
})

document.getElementById('nav-needs').addEventListener('click', () => {
  state.selectedRunId = null
  state.showNewTask = false
  state.phoneArea = 'side'
  render()
})

document.getElementById('new-task-toggle').addEventListener('click', () => {
  state.showNewTask = !state.showNewTask
  state.selectedRunId = null
  state.lockRemoved = null
  state.pendingDecision = null
  state.taskError = ''
  state.phoneArea = 'side'
  renderSide()
  if (state.showNewTask) {
    const field = document.getElementById('task-instructions')
    if (field) field.focus()
  }
})

for (const b of document.querySelectorAll('.tabs button')) {
  b.addEventListener('click', () => {
    state.phoneArea = b.dataset.tab
    renderPhoneTabs()
  })
}

document.getElementById('cancel-btn').addEventListener('click', async () => {
  if (!state.selectedSessionId) return
  try {
    await api(`/api/sessions/${encodeURIComponent(state.selectedSessionId)}/cancel`, {
      method: 'POST',
    })
    state.composerError = ''
    renderComposerError()
    await refreshAll()
  } catch (err) {
    state.composerError = `Could not stop the turn: ${err?.message || 'request failed'}.`
    renderComposerError()
  }
})

document.getElementById('composer-input').addEventListener('input', () => {
  if (!state.composerError) return
  state.composerError = ''
  renderComposerError()
})

document.getElementById('composer-form').addEventListener('submit', async (e) => {
  e.preventDefault()
  if (state.composerSending) return
  const sid = state.selectedSessionId
  const input = document.getElementById('composer-input')
  const text = input.value.trim()
  if (!sid || !text) return
  state.composerSending = true
  state.composerError = ''
  updateComposer(state.sessions.find((x) => x.id === sid) ?? null)
  try {
    await api(`/api/sessions/${encodeURIComponent(sid)}/messages`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ text }),
    })
    input.value = ''
    state.composerError = ''
    state.composerSending = false
    await loadSessionData(sid)
    await refreshAll()
  } catch (err) {
    state.composerSending = false
    state.composerError = `Not sent: ${err?.message || 'request failed'}. Your message is still here.`
    updateComposer(state.sessions.find((x) => x.id === state.selectedSessionId) ?? null)
  }
})

document.getElementById('composer-input').addEventListener('keydown', (e) => {
  if (e.key === 'Enter' && !e.shiftKey) {
    e.preventDefault()
    document.getElementById('composer-form').requestSubmit()
  }
})

let streamHasOpened = false
let streamWasUnreachable = false

function connectStream() {
  const es = new EventSource('/api/stream')
  es.onopen = async () => {
    const shouldReload = streamHasOpened || streamWasUnreachable
    streamHasOpened = true
    if (shouldReload) {
      try {
        state.hostUnreachable = false
        streamWasUnreachable = false
        await refreshAll()
      } catch {
        state.hostUnreachable = true
        streamWasUnreachable = true
        renderStatusBar()
      }
      return
    }
    streamWasUnreachable = false
    if (state.hostUnreachable) {
      state.hostUnreachable = false
      renderStatusBar()
    }
  }
  es.onerror = () => {
    streamWasUnreachable = true
    if (!state.hostUnreachable) {
      state.hostUnreachable = true
      renderStatusBar()
    }
    if (es.readyState === EventSource.CLOSED) {
      es.close()
      setTimeout(connectStream, 1000)
    }
  }
  es.onmessage = async (msg) => {
    if (!msg.data) return
    let ev
    try {
      ev = JSON.parse(msg.data)
    } catch {
      return
    }
    if (ev.type === 'session') {
      const prev = state.sessions.find((s) => s.id === ev.session.id)
      if (ev.session.frozenReason && !prev?.frozenReason) {
        state.arrivingLocks.add(ev.session.id)
      }
      await refreshAll()
    } else if (ev.type === 'run') {
      const prev = state.runs.find((r) => r.id === ev.run.id)
      if (ev.run.status === 'done' && prev && prev.status !== 'done') {
        state.freshDoneIds.add(`run:${ev.run.id}`)
      }
      await refreshAll()
    } else if (ev.type === 'message') {
      if (ev.message.sessionId === state.selectedSessionId) {
        await loadSessionData(ev.message.sessionId)
      }
      await refreshAll()
    } else if (ev.type === 'turn') {
      if (ev.turn.sessionId === state.selectedSessionId) {
        await loadSessionData(ev.turn.sessionId)
      }
      await refreshAll()
    } else if (ev.type === 'agent') {
      if (ev.sessionId === state.selectedSessionId) {
        const turns = state.turnsBySession.get(ev.sessionId) ?? []
        const t = turns.find((x) => x.id === ev.turnId)
        if (t) {
          t.events = t.events || []
          t.events.push(ev.event)
          renderStream()
        } else {
          await loadSessionData(ev.sessionId)
          renderStream()
        }
      }
    }
  }
}

refreshAll().then(() => {
  if (window.matchMedia('(max-width: 860px)').matches && attentionCount() > 0) {
    state.phoneArea = 'side'
    renderPhoneTabs()
    renderPhoneHead()
  }
  connectStream()
  setInterval(renderStatusBar, 10000)
})
