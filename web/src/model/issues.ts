// What the list and the board show, as pure functions over the API's types. No React here.
import type { components } from '@/api/schema'

type Schemas = components['schemas']
export type IssueRow = Schemas['IssueRow']
export type IssueList = Schemas['IssueList']
export type IssueWritten = Schemas['IssueWritten']
export type StreamItem = Schemas['StreamItem']
export type StatusInfo = Schemas['StatusInfo']
export type ErrorBody = Schemas['ErrorBody']
export type Actor = Schemas['Actor']
export type ActorInfo = Schemas['ActorInfo']
export type RunState = Schemas['RunSummary']['state']
export type ChecksState = Schemas['ChecksState']

export const AGENT_NAMES: Readonly<Record<string, string>> = {
  'claude-code': 'Claude Code',
  codex: 'Codex',
  opencode: 'OpenCode',
  pi: 'Pi',
  cursor: 'Cursor',
}

export const agentName = (kind: string | null | undefined): string => (kind ? (AGENT_NAMES[kind] ?? kind) : 'Agent')

export type AssigneeView =
  | { kind: 'unassigned'; label: 'Unassigned' }
  | { kind: 'person'; label: string; initials: string }
  | { kind: 'agent'; label: string; name: string }

export interface RunView {
  state: RunState
  label: string
  detail: string | null
}

export interface PullRequestView {
  state: 'none' | Schemas['PullRequestStatus']
  label: string
  checks: ChecksState
  url: string | null
  title: string
}

export interface GateView {
  text: string
  passed: number
  total: number
  title: string
}

export interface RowView {
  id: string
  title: string
  status: string
  statusName: string
  labels: string[]
  assignee: AssigneeView
  run: RunView
  pullRequest: PullRequestView
  gates: GateView
}

const initials = (name: string): string =>
  name
    .split(/\s+/)
    .filter(Boolean)
    .slice(0, 2)
    .map((word) => word[0]?.toUpperCase() ?? '')
    .join('')

export function assigneeView(actor: Actor | null | undefined): AssigneeView {
  if (!actor) return { kind: 'unassigned', label: 'Unassigned' }
  if (actor.role === 'agent') return { kind: 'agent', label: agentName(actor.kind), name: actor.name }
  return { kind: 'person', label: actor.name, initials: initials(actor.name) }
}

/** How long ago `at` was, as `45s`, `4m`, `3h` or `2d`. */
export function age(at: string, now: Date): string {
  const seconds = Math.max(0, Math.round((now.getTime() - Date.parse(at)) / 1000))
  if (seconds < 60) return `${String(seconds)}s`
  if (seconds < 3600) return `${String(Math.floor(seconds / 60))}m`
  if (seconds < 86400) return `${String(Math.floor(seconds / 3600))}h`
  return `${String(Math.floor(seconds / 86400))}d`
}

export function runView(run: IssueRow['run'], now: Date): RunView {
  switch (run.state) {
    case 'idle':
      return { state: 'idle', label: 'Idle', detail: null }
    case 'queued':
      return { state: 'queued', label: 'Queued', detail: `waiting for a runner that runs ${agentName(run.agent)}` }
    case 'running':
      return {
        state: 'running',
        label: `Running · ${age(run.since, now)}`,
        detail: `${agentName(run.agent)} is running`,
      }
    case 'failed':
      return { state: 'failed', label: 'Run failed', detail: run.reason ?? null }
  }
}

export function pullRequestView(pr: IssueRow['pull_request']): PullRequestView {
  if (!pr) return { state: 'none', label: 'no PR', checks: 'none', url: null, title: 'no pull request' }
  const label = `#${String(pr.number)} ${pr.state}`
  const checks = pr.checks === 'none' ? '' : `, checks ${pr.checks}`
  return { state: pr.state, label, checks: pr.checks, url: pr.url ?? null, title: `PR ${label}${checks}` }
}

export function gateView(gates: IssueRow['gates']): GateView {
  if (!gates || gates.total === 0)
    return { text: 'no gates', passed: 0, total: 0, title: 'no gated transition from here' }
  const failing = gates.first_failed ? `; first failing: ${gates.first_failed}` : ''
  return {
    text: `${String(gates.passed)} of ${String(gates.total)} gates pass`,
    passed: gates.passed,
    total: gates.total,
    title: `to ${gates.to_name}${failing}`,
  }
}

export function rowView(row: IssueRow, now: Date): RowView {
  const { issue } = row
  return {
    id: issue.id,
    title: issue.title,
    status: issue.status,
    statusName: issue.status_name,
    labels: issue.labels ?? [],
    assignee: assigneeView(issue.assignee),
    run: runView(row.run, now),
    pullRequest: pullRequestView(row.pull_request),
    gates: gateView(row.gates),
  }
}

/** A status no agent may act in carries the "people only" badge. */
export const peopleOnly = (status: StatusInfo): boolean => status.locked_to.includes('agent')

/** What a row shows for a refused request: the code and the rule, and for a gate the gate and its reason. */
export function refusalText(error: ErrorBody): string {
  if (error.code === 'gate_failed' && error.gate)
    return `Refused (gate_failed): gate ${error.gate} failed: ${error.reason ?? error.message}`
  return `Refused (${error.code}): ${error.message}`
}

// ---------- the transition a row waits on ----------

/** A status change is not optimistic: the row shows the request until the server answers. */
export type Transition =
  { state: 'pending'; to: string; toName: string } | { state: 'refused'; to: string; toName: string; text: string }

export type Transitions = Readonly<Record<string, Transition>>

// ---------- filters and grouping ----------

export type Who = '' | 'people' | 'agents' | 'none' | `agent:${string}`

export interface Filter {
  text: string
  who: Who
}

function matchesWho(actor: Actor | null | undefined, who: Who): boolean {
  if (who === '') return true
  if (who === 'none') return !actor
  if (!actor) return false
  if (who === 'people') return actor.role === 'person'
  if (who === 'agents') return actor.role === 'agent'
  return actor.role === 'agent' && `agent:${actor.kind ?? ''}` === who
}

export function filterRows(rows: readonly IssueRow[], filter: Filter): IssueRow[] {
  const text = filter.text.trim().toLowerCase()
  return rows.filter(({ issue }) => {
    if (!matchesWho(issue.assignee, filter.who)) return false
    if (!text) return true
    const assignee = issue.assignee ? `${issue.assignee.name} ${agentName(issue.assignee.kind)}` : ''
    const haystack = `${issue.id} ${issue.title} ${(issue.labels ?? []).join(' ')} ${assignee} ${issue.status_name}`
    return haystack.toLowerCase().includes(text)
  })
}

export interface Group {
  status: StatusInfo
  rows: IssueRow[]
}

/** Every workflow status in order, each with its rows in list order. */
export function groupByStatus(rows: readonly IssueRow[], statuses: readonly StatusInfo[]): Group[] {
  const by = new Map<string, IssueRow[]>(statuses.map((status) => [status.key, []]))
  for (const row of rows) by.get(row.issue.status)?.push(row)
  return statuses.map((status) => ({ status, rows: by.get(status.key) ?? [] }))
}

/** The status `delta` columns from `key` on the board, or null past either end. */
export function neighbourStatus(statuses: readonly StatusInfo[], key: string, delta: number): StatusInfo | null {
  const index = statuses.findIndex((status) => status.key === key)
  return index < 0 ? null : (statuses[index + delta] ?? null)
}

// ---------- the cache the stream keeps current (ADR 0006) ----------

export interface IssueCache {
  rows: IssueRow[]
  /** The last stream event applied. */
  lastSeq: number
  /** Per issue, the newest event its row is known to include, so an older answer never overwrites a newer row. */
  rowSeq: Readonly<Record<string, number>>
}

export function fromList(list: IssueList): IssueCache {
  return {
    rows: list.issues,
    lastSeq: list.last_event_seq,
    rowSeq: Object.fromEntries(list.issues.map((row) => [row.issue.id, list.last_event_seq])),
  }
}

function upsert(cache: IssueCache, row: IssueRow, seq: number): IssueCache {
  const id = row.issue.id
  if ((cache.rowSeq[id] ?? -1) > seq) return cache
  const index = cache.rows.findIndex((r) => r.issue.id === id)
  const rows = index < 0 ? [...cache.rows, row] : cache.rows.with(index, row)
  return { ...cache, rows, rowSeq: { ...cache.rowSeq, [id]: seq } }
}

/** A stream message: ignored at or below the last sequence applied. */
export function applyStream(cache: IssueCache, item: StreamItem): IssueCache {
  if (item.event.seq <= cache.lastSeq) return cache
  return { ...upsert(cache, item.row, item.event.seq), lastSeq: item.event.seq }
}

/** A write's answer: its row, unless the cache already holds a newer one. */
export const applyWrite = (cache: IssueCache, written: IssueWritten): IssueCache =>
  upsert(cache, written.row, written.seq)

/** An optimistic change to one row, or putting a row back on rollback. Leaves `rowSeq` alone. */
export function replaceRow(cache: IssueCache, id: string, change: (row: IssueRow) => IssueRow): IssueCache {
  const index = cache.rows.findIndex((r) => r.issue.id === id)
  const row = cache.rows[index]
  return row ? { ...cache, rows: cache.rows.with(index, change(row)) } : cache
}
