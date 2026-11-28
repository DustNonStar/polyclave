// Single client-side state object. Views read it; app.ts owns every mutation.
import type { Connection } from '../api/sse.js';
import type { HubMessage, Profile, ProviderUsage, RunDetail, TicketIntent } from '../api/client.js';
import type {
  CouncilDto, CuaLockDto, EvolutionSuggestionDto, LocalSkillDto, MemoryItemDto, MeshNodeDto, MeshNodeLimitDto,
  NotificationChannelDto, PairedDeviceDto, PairingTicketDto, PauseStateDto, PrGateDto, RunDto,
  SessionDto, Vendor,
} from '../generated/proto.js';

export type Panel = 'Topology' | 'Review' | 'Files' | 'Memory';
export type ReviewView = 'runs' | 'council' | 'pr_gates';
export type Pane = 'work' | 'chat' | 'inspect';

export interface LiveTurn { text: string; reasoning: string }

export const S = {
  connection: null as Connection | null,
  online: false,
  sessions: [] as SessionDto[],
  runs: [] as RunDto[],
  tickets: [] as TicketIntent[],
  profiles: [] as Profile[],
  usage: [] as ProviderUsage[],
  memory: [] as MemoryItemDto[],
  messages: [] as HubMessage[],
  councils: [] as CouncilDto[],
  prGates: [] as PrGateDto[],
  pause: { paused: false, reason: null, updated_at: '' } as PauseStateDto,
  devices: [] as PairedDeviceDto[],
  meshNodes: [] as MeshNodeDto[],
  meshLimits: [] as MeshNodeLimitDto[],
  notificationChannels: [] as NotificationChannelDto[],
  lastPairing: null as { ticket: PairingTicketDto; secret: string; origin: string | null; qr_uri: string | null } | null,
  cua: { locked: false, holder_run_id: null, target_window: null, idle_threshold_secs: 30, acquired_at: null, reclaimed_at: null } as CuaLockDto,
  evolutionSuggestions: [] as EvolutionSuggestionDto[],
  localSkills: [] as LocalSkillDto[],
  /** Selected Hub session; null with global=true shows the cross-project Global Hub. */
  selected: null as string | null,
  global: false,
  activeRun: null as string | null,
  activeCouncil: null as string | null,
  activePrGate: null as string | null,
  reviewView: 'runs' as ReviewView,
  panel: 'Topology' as Panel,
  pane: 'chat' as Pane,
  /** Streaming text by session or run id (memory only, never persisted). */
  live: new Map<string, LiveTurn>(),
  details: new Map<string, RunDetail>(),
  collapsed: new Set<string>(),
  memFilter: 'all',
  filePath: '',
  fileText: null as string | null,
  fileError: '',
};

export const selectedSession = (): SessionDto | undefined => S.sessions.find(s => s.id === S.selected);
export const hubSessions = (): SessionDto[] => S.sessions.filter(s => s.role !== 'worker');
export const sessionRuns = (id: string | null = S.selected): RunDto[] => S.runs.filter(r => (id ? r.hub_session_id === id : true));
export const visibleRuns = (): RunDto[] => (S.global ? S.runs : sessionRuns());
export const needsReview = (): RunDto[] => S.runs.filter(r => r.status === 'needs_review');
export const awaitingCouncils = (): CouncilDto[] => S.councils.filter(c => c.status === 'awaiting_user');
export const attentionPrGates = (): PrGateDto[] => S.prGates.filter(g => g.status === 'scope_violation' || g.status === 'escalated' || g.status === 'failed');
export const totalNeedsCount = (): number => needsReview().length + awaitingCouncils().length + attentionPrGates().length;
export const activeRunDto = (): RunDto | undefined =>
  S.runs.find(r => r.id === S.activeRun) ?? visibleRuns().find(r => r.status === 'needs_review') ?? visibleRuns().slice(-1)[0];

export function vendorLabel(v: Vendor | string): string {
  switch (v) {
    case 'codex': return 'Codex';
    case 'claude': return 'Claude';
    case 'agy': return 'Antigravity';
    case 'dsh': return 'DSH';
    default: return v;
  }
}

export type Tone = 'run' | 'review' | 'done' | 'fail' | 'think' | 'archive' | 'idle';
export function runTone(status: string): Tone {
  switch (status) {
    case 'running': case 'pending': return 'run';
    case 'needs_review': case 'interrupted': return 'review';
    case 'done': return 'done';
    case 'failed': return 'fail';
    default: return 'idle';
  }
}
export const runLabel = (status: string): string => ({
  pending: 'Queued', running: 'Running', needs_review: 'Needs review', interrupted: 'Interrupted', done: 'Verified', failed: 'Failed',
} as Record<string, string>)[status] ?? status.replaceAll('_', ' ');

/** Map a profile or provider label onto one of the four quota vendors. */
export function vendorOf(text: string | null | undefined): 'claude' | 'codex' | 'gemini' | 'deepseek' {
  const t = (text ?? '').toLowerCase();
  if (t.includes('deepseek') || t === 'dsh' || t.includes('dsh')) return 'deepseek';
  if (t.includes('claude') || t.includes('anthropic')) return 'claude';
  if (t.includes('gemini') || t === 'agy' || t.includes('antigravity')) return 'gemini';
  return 'codex';
}

export interface Actions {
  selectSession(id: string): void;
  selectGlobal(): void;
  openRun(id: string, focusPanel?: boolean): void;
  setPanel(p: Panel): void;
  setPane(p: Pane): void;
  rerender(): void;
  tell(message: string, tone?: 'error' | 'info'): void;
  problem(e: unknown): void;
  refresh(): Promise<void>;
}
