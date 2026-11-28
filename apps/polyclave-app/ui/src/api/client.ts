// Typed wrappers for the local daemon API. Every call goes through the authenticated loopback connection.
import { request } from './sse.js';
import type { PrGateDiffDto } from '../generated/proto.js';
import type { Connection } from './sse.js';
import type {
  ClientKind, CommunityDraftDto, CouncilDto, CouncilLevel, CuaLockDto, EvolutionSuggestionDto,
  JsonValue, LocalSkillDto, MemoryItemDto, MemoryKind, MeshNodeDto, MeshNodeLimitDto, NotificationChannelDto,
  NotificationChannelKind, PairedDeviceDto, PairingTicketDto, PauseStateDto, PlanningPreference,
  PrGateDto, RunDto, RunEvidenceDto, SessionDto, Vendor,
} from '../generated/proto.js';

export interface Profile { id: string; display_name: string; kind: string; models: string[] }
/** Hub message as the daemon serializes it (`metadata`); older fixtures may use `metadata_json`. */
export interface HubMessage {
  id: number; turn_id?: number | null; source: 'user' | 'agent' | 'host'; text: string;
  reasoning?: string | null; metadata?: JsonValue | null; metadata_json?: JsonValue | null; queued: boolean;
  provider_label?: string | null; created_at?: string;
}
export interface TicketIntent { id: string; hub: string; repo: string; used: number; limit: number; status: string }
export interface CommitSummary { files: string; commits: string }
/** `evidence` is absent from daemons older than the approval-evidence change. */
export interface RunDetail { run: RunDto; commit: CommitSummary | null; evidence?: RunEvidenceDto | null }
export interface ProviderUsage {
  vendor: string; tokens?: number | null; cost?: number | null; context?: number | null; turns?: number | null;
  five_hour?: number | null; weekly?: number | null; balance?: number | null; series?: number[] | null;
}
export interface Mandate { goal: string; allowed_paths: string[]; forbidden_actions: string[]; acceptance_criteria: string[] }
export interface RunRequest {
  hub_session_id: string; request_id: string; vendor: Vendor; model: string | null; repo_cwd: string;
  instructions: string; base_branch: string; ticket_id: string | null;
}
export type Decision = 'accept' | 'fail' | 'allow_retry';

const enc = encodeURIComponent;

export function api(connection: () => Connection | null) {
  const call = <T>(path: string, body?: unknown): Promise<T> => {
    const c = connection();
    return c ? request<T>(c, path, body) : Promise.reject(Error('Connect to the local daemon first'));
  };
  const s = (id: string) => '/api/sessions/' + enc(id);
  const r = (id: string) => '/api/runs/' + enc(id);
  return {
    sessions: () => call<SessionDto[]>('/api/sessions'),
    register: (name: string, repo_cwd: string) => call<SessionDto>('/api/sessions', { name, repo_cwd }),
    profiles: () => call<Profile[]>('/api/profiles'),
    switchProfile: (id: string, profile_id: string, model: string) => call<{ updated: boolean }>(s(id) + '/profile', { profile_id, model }),
    messages: (id: string) => call<HubMessage[]>(s(id) + '/messages'),
    send: (id: string, text: string) => call<HubMessage>(s(id) + '/messages', { request_id: crypto.randomUUID(), text }),
    cancelTurn: (id: string) => call<{ accepted: boolean }>(s(id) + '/cancel', {}),
    tickets: () => call<TicketIntent[]>('/api/tickets'),
    createTicket: (id: string, message_id: number, title: string, mandate: Mandate, max_runs: number) =>
      call<TicketIntent>(s(id) + '/tickets', { message_id, title, mandate, max_runs }),
    runs: () => call<RunDto[]>('/api/runs'),
    dispatch: (req: RunRequest) => call<RunDto>('/api/runs', req),
    run: (id: string) => call<RunDetail>(r(id)),
    resolve: (id: string, decision: Decision, note: string, seen_sha?: string | null) =>
      call<RunDto>(r(id) + '/resolve', { decision, note, seen_sha: seen_sha ?? null }),
    retry: (id: string) => call<{ accepted: boolean }>(r(id) + '/retry', {}),
    cancelRun: (id: string) => call<{ accepted: boolean }>(r(id) + '/cancel', {}),
    workerChat: (id: string, text: string) => call<{ accepted: boolean }>(r(id) + '/worker-chat', { text }),
    repoFile: (id: string, path: string) => call<{ text: string }>(s(id) + '/repo-file', { path }),
    memory: (id: string) => call<MemoryItemDto[]>(s(id) + '/memory'),
    draftMemory: (id: string, kind: MemoryKind, title: string, content: string, source: { message_id: number } | { run_id: string }) =>
      call<MemoryItemDto>(s(id) + '/memory', { action: 'message_id' in source ? 'draft' : 'draft_run', ...source, kind, title, content }),
    commitMemory: (id: string, memoryId: string) => call<MemoryItemDto>(s(id) + '/memory', { action: 'commit', id: memoryId }),
    archiveMemory: (id: string, memoryId: string) => call<MemoryItemDto>(s(id) + '/memory', { action: 'archive', id: memoryId }),
    usage: () => call<ProviderUsage[]>('/api/usage'),
    councils: () => call<CouncilDto[]>('/api/councils'),
    startCouncil: (body: {
      hub_session_id?: string; ticket_id?: string | null; repo_cwd?: string;
      level?: CouncilLevel; preference?: PlanningPreference; prompt: string;
    }) => call<CouncilDto>('/api/councils', body),
    resolveCouncil: (id: string, chosen_label: string, decision_note?: string | null) =>
      call<CouncilDto>('/api/councils/' + enc(id) + '/resolve', { chosen_label, decision_note: decision_note || null }),
    prGates: () => call<PrGateDto[]>('/api/pr-gates'),
    prGate: (id: string) => call<PrGateDto>('/api/pr-gates/' + enc(id)),
    prGateDiff: (id: string) => call<PrGateDiffDto>('/api/pr-gates/' + enc(id) + '/diff'),
    approvePrGate: (id: string, expected_sha: string) =>
      call<PrGateDto>('/api/pr-gates/' + enc(id) + '/approve', { expected_sha }),
    runPrGate: (id: string, body?: Record<string, unknown>) => call<PrGateDto>('/api/pr-gates/' + enc(id) + '/run', body ?? {}),
    pauseState: () => call<PauseStateDto>('/api/pause'),
    pause: (reason?: string | null) => call<PauseStateDto>('/api/pause', reason ? { reason } : {}),
    resume: () => call<PauseStateDto>('/api/resume', {}),
    pairingTickets: () => call<PairingTicketDto[]>('/api/pairing/tickets'),
    createPairingTicket: (client_kind: ClientKind, device_label: string, ttl_secs = 600) =>
      call<{ ticket: PairingTicketDto; secret: string; origin: string | null; qr_uri: string | null }>('/api/pairing/tickets', { client_kind, device_label, ttl_secs }),
    devices: () => call<PairedDeviceDto[]>('/api/devices'),
    revokeDevice: (id: string) => call<PairedDeviceDto>('/api/devices/' + enc(id) + '/revoke', {}),
    meshNodes: () => call<MeshNodeDto[]>('/api/mesh/nodes'),
    revokeMeshNode: (id: string) => call<MeshNodeDto>('/api/mesh/nodes/' + enc(id) + '/revoke', {}),
    meshLimits: () => call<MeshNodeLimitDto[]>('/api/mesh/limits'),
    setMeshConcurrency: (id: string, max_concurrent_runs: number) =>
      call<MeshNodeLimitDto>('/api/mesh/nodes/' + enc(id) + '/concurrency', { max_concurrent_runs }),
    notificationChannels: () => call<NotificationChannelDto[]>('/api/notifications/channels'),
    upsertNotificationChannel: (body: { kind: NotificationChannelKind; label: string; endpoint: string; topic: string; enabled?: boolean }) =>
      call<NotificationChannelDto>('/api/notifications/channels', body),
    testNotifications: () => call<{ delivered: number }>('/api/notifications/test', {}),
    cua: () => call<CuaLockDto>('/api/cua'),
    reclaimCua: () => call<CuaLockDto>('/api/cua/reclaim', {}),
    evolutionSuggestions: () => call<EvolutionSuggestionDto[]>('/api/evolution/suggestions'),
    distillEvolution: () => call<EvolutionSuggestionDto[]>('/api/evolution/distill', {}),
    applyEvolution: (id: string) => call<EvolutionSuggestionDto>('/api/evolution/suggestions/' + enc(id) + '/apply', {}),
    rollbackEvolution: (id: string) => call<EvolutionSuggestionDto>('/api/evolution/suggestions/' + enc(id) + '/rollback', {}),
    localSkills: () => call<LocalSkillDto[]>('/api/skills'),
    saveLocalSkill: (body: { name: string; description: string; body_md: string; synced_vendors?: Vendor[] }) =>
      call<LocalSkillDto>('/api/skills', body),
    rollbackLocalSkill: (id: string) => call<LocalSkillDto>('/api/skills/' + enc(id) + '/rollback', {}),
    communityDraft: () => call<CommunityDraftDto>('/api/community/draft'),
    setCommunityOptIn: (opt_in: boolean) => call<{ opt_in: boolean }>('/api/community/opt-in', { opt_in }),
  };
}
export type Api = ReturnType<typeof api>;
