// What the issue page shows, as pure functions over the API's types. No React here.
import type { components } from '@/api/schema'
import { agentName } from '@/model/issues'

type Schemas = components['schemas']
export type Issue = Schemas['Issue']
export type IssueEvent = Schemas['Event']
export type Run = Schemas['Run']
export type AttemptInfo = Schemas['AttemptInfo']
export type Proof = Schemas['Proof']
export type PullRequest = Schemas['PullRequest']
export type CheckRun = Schemas['CheckRunEvidence']
export type ReviewThread = Schemas['ReviewThread']
export type GateChecklist = Schemas['GateChecklist']
export type GateResult = Schemas['GateResult']
export type TransitionList = Schemas['TransitionList']
export type TransitionOption = Schemas['TransitionOption']
export type LogLine = Schemas['LogLine']
export type LinkedIssue = Schemas['LinkedIssue']

export interface GateSummary {
  text: string
  passed: number
  total: number
  /** The status the gated transition moves to, as people read it. */
  toName: string | null
  firstFailed: string | null
}

/** The gate checklist's headline: `1 of 4 gates pass`, and the first gate that fails. */
export function gateSummary(checklist: GateChecklist | null | undefined): GateSummary {
  if (!checklist || checklist.gates.length === 0)
    return { text: 'No gates on the next move', passed: 0, total: 0, toName: null, firstFailed: null }
  const passed = checklist.gates.filter((gate) => gate.passed).length
  const total = checklist.gates.length
  return {
    text: `${String(passed)} of ${String(total)} gates pass`,
    passed,
    total,
    toName: checklist.to_name,
    firstFailed: checklist.gates.find((gate) => !gate.passed)?.gate ?? null,
  }
}

/** A gate result's reason with the names it is about, as `GateResult::text()` joins them. */
export function gateText(result: Pick<GateResult, 'reason' | 'outside' | 'outside_more'>): string {
  const named = [...(result.outside ?? [])]
  if (result.outside_more) named.push(`and ${String(result.outside_more)} more`)
  return named.length > 0 ? `${result.reason}: ${named.join(', ')}` : result.reason
}

/** The page section each gate reads its evidence from, by element id. */
export const GATE_EVIDENCE: Readonly<Record<string, string>> = {
  pr_open: 'pr',
  pr_ready: 'pr',
  pr_names_issue: 'pr',
  ci_green: 'checks',
  mergeable: 'pr',
  no_open_threads: 'threads',
  proof_attached: 'proof',
}

type Links = Pick<Issue, 'blocked_by' | 'blocks'>

export interface Dependency {
  label: 'Blocked by' | 'Blocks'
  issues: LinkedIssue[]
}

/** The rail's two dependency lists, or none when the issue neither blocks nor is blocked. */
export function dependencies(issue: Links): Dependency[] {
  const blockedBy = issue.blocked_by ?? []
  const blocks = issue.blocks ?? []
  if (blockedBy.length === 0 && blocks.length === 0) return []
  return [
    { label: 'Blocked by', issues: blockedBy },
    { label: 'Blocks', issues: blocks },
  ]
}

/** Whether `issue` blocks or is blocked by `id`: an event on either can change what the other's page shows. */
export const linksTo = (issue: Links | undefined, id: string): boolean =>
  issue !== undefined && [...(issue.blocked_by ?? []), ...(issue.blocks ?? [])].some((linked) => linked.id === id)

export type Mark = 'person' | 'agent' | 'runner' | 'github' | 'server'

export interface ActorMark {
  mark: Mark
  /** The name the entry shows. */
  name: string
  /** The chip beside the name: the agent's kind, `runner`, `GitHub` or `server`; none for a person. */
  chip: string | null
  initials: string
}

const initials = (name: string): string =>
  name
    .split(/\s+/)
    .filter(Boolean)
    .slice(0, 2)
    .map((word) => word[0]?.toUpperCase() ?? '')
    .join('')

/**
 * How an activity entry marks who wrote it. A refusal is the server's: it refused a move that GitHub's event
 * would have made or an agent asked for, so the entry is marked as the server's and names that actor in its text.
 */
export function actorMark(event: Pick<IssueEvent, 'actor' | 'detail'>): ActorMark {
  const { actor } = event
  if (event.detail.kind === 'refusal') return { mark: 'server', name: 'Punchlist', chip: 'server', initials: 'PL' }
  switch (actor.role) {
    case 'person':
      return { mark: 'person', name: actor.name, chip: null, initials: initials(actor.name) }
    case 'agent':
      return { mark: 'agent', name: actor.name, chip: agentName(actor.kind), initials: '' }
    case 'runner':
      return { mark: 'runner', name: actor.name, chip: 'runner', initials: '' }
    case 'github':
      return { mark: 'github', name: actor.name, chip: 'GitHub', initials: '' }
  }
}

export type Feed = 'all' | 'comments' | 'transitions'

/** The activity entries a filter shows, oldest first. Transitions include the refused ones. */
export function feedEntries(events: readonly IssueEvent[], feed: Feed): IssueEvent[] {
  if (feed === 'comments') return events.filter((event) => event.detail.kind === 'comment')
  if (feed === 'transitions')
    return events.filter((event) => event.detail.kind === 'transition' || event.detail.kind === 'refusal')
  return [...events]
}

/** The words of an entry that is not a comment or a move. */
export function eventText(event: IssueEvent): string {
  const { detail } = event
  switch (detail.kind) {
    case 'created':
      return 'created the issue'
    case 'edited': {
      const fields = detail.fields
      const list = fields.length > 1 ? `${fields.slice(0, -1).join(', ')} and ${fields.at(-1) ?? ''}` : fields.join('')
      return `edited the ${list}`
    }
    case 'transition':
      return `moved ${detail.from_name} → ${detail.to_name}`
    case 'refusal':
      return `refused ${event.actor.name}'s move to ${detail.to_name}: gate ${detail.gate} failed: ${detail.reason}`
    case 'comment':
      return detail.body
    default:
      return detail.kind
  }
}

export type MoveState = 'allowed' | 'blocked' | 'locked'

export interface MoveView {
  to: string
  toName: string
  /** People only: the target status locks agents out. */
  peopleOnly: boolean
  by: string
  state: MoveState
  /** `allowed`: what passes. `blocked`: the gate's reason. `locked`: the rule. */
  text: string
  gate: string | null
  /** The names the blocked gate is about, such as the failing checks. */
  names: string[]
  disabled: boolean
}

/**
 * A listed transition as the rail shows it. A viewer the status locks out sees every move disabled. A move
 * the caller's role may not make is shown disabled when it is the next gated move and a gate fails, so a
 * person watching an agent's move sees what blocks it; otherwise it is null, and the page hides it.
 */
export function moveView(
  option: TransitionOption,
  viewerLocked: boolean,
  checklist: GateChecklist | null | undefined,
): MoveView | null {
  const { verdict } = option
  const base = {
    to: option.to.key,
    toName: option.to.name,
    peopleOnly: option.to.locked_to.includes('agent'),
    by: `by ${option.by.join(' or ')}`,
  }
  if (verdict.kind === 'refused' && verdict.code === 'role_not_allowed') {
    const failed = checklist?.to === option.to.key ? checklist.gates.find((gate) => !gate.passed) : undefined
    if (!failed) return null
    const names = [...(failed.outside ?? [])]
    if (failed.outside_more) names.push(`and ${String(failed.outside_more)} more`)
    return { ...base, state: 'blocked', text: failed.reason, gate: failed.gate, names, disabled: true }
  }
  if (verdict.kind === 'blocked') {
    const names = [...(verdict.outside ?? [])]
    if (verdict.outside_more) names.push(`and ${String(verdict.outside_more)} more`)
    // The verdict reads the recorded evidence; the request reads GitHub first, so it may pass where this fails.
    return { ...base, state: 'blocked', text: verdict.reason, gate: verdict.gate, names, disabled: false }
  }
  if (verdict.kind === 'refused' || viewerLocked)
    return {
      ...base,
      state: 'locked',
      text: verdict.kind === 'refused' ? verdict.message : 'this status is locked to your role',
      gate: null,
      names: [],
      disabled: true,
    }
  const gates = checklist?.to === option.to.key ? checklist.gates.length : 0
  return {
    ...base,
    state: 'allowed',
    text: gates > 0 ? `All ${String(gates)} gates pass.` : 'No gates on this move.',
    gate: null,
    names: [],
    disabled: false,
  }
}

/** `4m 12s`, `38s` or `1h 3m`. */
export function duration(ms: number | null | undefined): string {
  if (ms === null || ms === undefined) return '—'
  const seconds = Math.round(ms / 1000)
  if (seconds < 60) return `${String(seconds)}s`
  const minutes = Math.floor(seconds / 60)
  if (minutes < 60) return `${String(minutes)}m ${String(seconds % 60)}s`
  return `${String(Math.floor(minutes / 60))}h ${String(minutes % 60)}m`
}

const count = (n: number): string => (n < 1000 ? String(n) : `${(n / 1000).toFixed(1)}k`)

/** `12.4k in · 3.1k out`, or `—` when the runner reported none. */
export function tokens(input: number | null | undefined, output: number | null | undefined): string {
  if (input === null || input === undefined || output === null || output === undefined) return '—'
  return `${count(input)} in · ${count(output)} out`
}

/** The runs rail's line for an issue that waits to be claimed, with the server's reason; null when it does not wait. */
export const waitingLine = (issue: Pick<Issue, 'waiting_to_run'>): string | null =>
  issue.waiting_to_run ? `Waiting: ${issue.waiting_to_run}` : null

/**
 * The cost the agent reported, as the server stored it: no rounding, so no float. Null when it
 * reported none (Codex), which is not `$0`.
 */
export function cost(value: string | null | undefined): string | null {
  return value === null || value === undefined ? null : `$${value}`
}

/**
 * The model an attempt ran on, as `pl run list` says it: the model the agent reported, beside
 * what the runner pinned, if anything. `name` is null when there is no model to name, and
 * `note` then stands alone.
 */
export function modelView(
  attempt: Pick<AttemptInfo, 'model' | 'reported_model' | 'state'>,
): { name: string | null; note: string } {
  const pinned = attempt.model ?? null
  const reported = attempt.reported_model ?? null
  if (reported !== null) {
    if (pinned === null) return { name: reported, note: 'agent default' }
    return { name: reported, note: pinned === reported ? 'pinned' : `pinned ${pinned}` }
  }
  if (attempt.state === 'running') {
    return { name: null, note: pinned === null ? 'waiting for the agent' : `waiting for the agent, pinned ${pinned}` }
  }
  return pinned === null ? { name: null, note: 'agent default' } : { name: pinned, note: 'pinned' }
}

/** A pull request's URL when it is a plain `https://github.com/<owner>/<repo>/pull/<number>`, as the rows' is. */
export function plainPullRequestUrl(number: number, url: string): string | null {
  const parts = /^https:\/\/github\.com\/([\w.-]+)\/([\w.-]+)\/pull\/(\d+)$/.exec(url)
  const plain = (part: string | undefined) => part !== undefined && part !== '.' && part !== '..'
  return parts && plain(parts[1]) && plain(parts[2]) && parts[3] === String(number) ? url : null
}

/** The pull request panel's notice for an issue that waits on the merge, as `waitsOnMerge` decides. */
export const mergeNotice = (id: string, to: string, mergeState: string): string =>
  `Waits on the merge. ${id} moves to ${to} when GitHub reports it.${mergeState === 'dirty' ? ' The pull request has conflicts.' : ''}`

/** A review comment's URL when it is a plain `https://github.com/<owner>/<repo>/pull/<number>#discussion_r<digits>`. */
export function plainThreadUrl(pullRequest: number, url: string | null | undefined): string | null {
  const parts = /^(.*)#discussion_r(\d+)$/.exec(url ?? '')
  return parts?.[1] !== undefined && plainPullRequestUrl(pullRequest, parts[1]) ? parts[0] : null
}

export interface ThreadView {
  /** `path:line`, the path alone without a line, or `no file`. */
  place: string
  /** `last 10 of 14 comments` when the thread has more than are recorded. */
  note: string | null
  comments: { author: string; body: string; cut: boolean }[]
  /** No comment is recorded, such as for a thread an event without comments brought. */
  unread: boolean
  url: string | null
}

/** What a review thread shows. Its path, authors and bodies are text from outside the team: render them as text. */
export function threadView(thread: ReviewThread): ThreadView {
  const { path, line, comments, comments_total: total } = thread
  return {
    place: path == null ? 'no file' : line == null ? path : `${path}:${String(line)}`,
    note: total > comments.length ? `last ${String(comments.length)} of ${String(total)} comments` : null,
    comments: comments.map(({ author, body, cut }) => ({ author, body, cut: cut ?? false })),
    unread: comments.length === 0,
    url: plainThreadUrl(thread.pull_request, thread.url),
  }
}

/** The content type the server takes for each proof extension (`PROOF_TYPES` in `punchlist-api`). */
export const PROOF_TYPES: Readonly<Record<string, string>> = {
  png: 'image/png',
  jpg: 'image/jpeg',
  jpeg: 'image/jpeg',
  gif: 'image/gif',
  webp: 'image/webp',
  mp4: 'video/mp4',
  webm: 'video/webm',
  mov: 'video/quicktime',
  txt: 'text/plain',
  log: 'text/plain',
  md: 'text/markdown',
  xml: 'application/xml',
  pdf: 'application/pdf',
}

/** A file's proof content type from its extension, in any case, or `null` when proof cannot be one. */
export function proofType(name: string): string | null {
  const dot = name.lastIndexOf('.')
  return dot < 0 ? null : (PROOF_TYPES[name.slice(dot + 1).toLowerCase()] ?? null)
}

/** The image or video proof `delta` steps from `id` in card order, skipping reports; `null` past either end. */
export function mediaStep(proof: readonly Pick<Proof, 'id' | 'kind'>[], id: string, delta: 1 | -1): string | null {
  const media = proof.filter((item) => item.kind !== 'report')
  const at = media.findIndex((item) => item.id === id)
  return at < 0 ? null : (media[at + delta]?.id ?? null)
}

/** `512 B`, `12 KB` or `3.4 MB`. */
export function fileSize(bytes: number): string {
  if (bytes < 1024) return `${String(bytes)} B`
  if (bytes < 1024 * 1024) return `${String(Math.round(bytes / 1024))} KB`
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`
}

export type SpecBlock =
  | { kind: 'heading'; text: string }
  | { kind: 'item'; text: string }
  | { kind: 'check'; text: string; done: boolean }
  | { kind: 'text'; text: string }

/**
 * An issue body as the spec panel shows it: headings, list items, checklist items (`- [ ]` and `- [x]`, the
 * acceptance criteria) and paragraphs. Anything else of Markdown stays as written.
 */
export function specBlocks(body: string): SpecBlock[] {
  const blocks: SpecBlock[] = []
  for (const raw of body.split('\n')) {
    const line = raw.trim()
    if (!line) continue
    const heading = /^#{1,6}\s+(.*)$/.exec(line)
    const check = /^[-*]\s+\[([ xX])\]\s+(.*)$/.exec(line)
    const item = /^(?:[-*]|\d+\.)\s+(.*)$/.exec(line)
    if (heading) blocks.push({ kind: 'heading', text: heading[1] ?? '' })
    else if (check) blocks.push({ kind: 'check', text: check[2] ?? '', done: check[1] !== ' ' })
    else if (item) blocks.push({ kind: 'item', text: item[1] ?? '' })
    else {
      const last = blocks.at(-1)
      if (last?.kind === 'text') last.text = `${last.text} ${line}`
      else blocks.push({ kind: 'text', text: line })
    }
  }
  return blocks
}

/** What a run log line says, as the log box shows it: `plain` is a line that is not a JSON object, shown as written. */
export type LogKind = 'message' | 'tool' | 'result' | 'error' | 'event' | 'plain'

export interface LogView {
  kind: LogKind
  text: string
}

type Json = Record<string, unknown>
const obj = (value: unknown): Json =>
  typeof value === 'object' && value !== null && !Array.isArray(value) ? (value as Json) : {}
const str = (value: unknown): string => (typeof value === 'string' ? value : '')
const list = (value: unknown): unknown[] => (Array.isArray(value) ? value : [])

/** The first non-empty line of `text`, cut at `max` characters; `…` marks a cut. */
function clip(text: string, max = 200): string {
  const lines = text.trim().split('\n')
  const first = lines[0] ?? ''
  return first.length > max ? `${first.slice(0, max)}…` : lines.length > 1 ? `${first} …` : first
}

/**
 * One line of a run's log as a short summary: Claude Code's `stream-json` (an assistant's text, a tool call and
 * its target, a tool's result) or Codex's app-server messages (an item, token use, the turn's end). Anything
 * else is shown as written.
 */
export function logView(line: string): LogView {
  let parsed: unknown
  try {
    parsed = JSON.parse(line)
  } catch {
    return { kind: 'plain', text: line }
  }
  if (typeof parsed !== 'object' || parsed === null || Array.isArray(parsed)) return { kind: 'plain', text: line }
  const message = parsed as Json
  return typeof message.type === 'string' ? claudeLine(message) : codexLine(message)
}

const TOOL_TARGETS = ['file_path', 'notebook_path', 'path', 'command', 'pattern', 'url', 'query', 'description']

function toolCall(name: string, input: Json): string {
  const target = TOOL_TARGETS.map((key) => str(input[key])).find(Boolean)
  return target ? `${name} ${clip(target, 300)}` : name
}

function claudeLine(line: Json): LogView {
  const content = list(obj(line.message).content).map(obj)
  switch (line.type) {
    case 'system':
      return { kind: 'event', text: line.subtype === 'init' ? `session started, model ${str(line.model)}` : `system ${str(line.subtype)}` }
    case 'assistant':
      return {
        kind: content.some((block) => block.type === 'tool_use') ? 'tool' : 'message',
        text: content
          .map((block) =>
            block.type === 'text' ? str(block.text) : block.type === 'tool_use' ? toolCall(str(block.name), obj(block.input)) : str(block.type),
          )
          .join('\n'),
      }
    case 'user': {
      const results = content.filter((block) => block.type === 'tool_result')
      if (results.length === 0) return { kind: 'event', text: 'user message' }
      const said = (block: Json) =>
        typeof block.content === 'string' ? block.content : list(block.content).map((part) => str(obj(part).text)).join('\n')
      return {
        kind: results.some((block) => block.is_error === true) ? 'error' : 'result',
        text: results.map((block) => `${block.is_error === true ? 'error' : 'result'}: ${clip(said(block))}`).join('\n'),
      }
    }
    case 'result':
      return {
        kind: line.is_error === true ? 'error' : 'result',
        text: `finished ${str(line.subtype)}${str(line.result) ? `: ${str(line.result)}` : ''}`,
      }
    default:
      return { kind: 'event', text: str(line.type) }
  }
}

function codexLine(line: Json): LogView {
  const method = str(line.method)
  const params = obj(line.params)
  if (method === 'item/started' || method === 'item/completed') return codexItem(obj(params.item), method === 'item/completed')
  if (method === 'thread/tokenUsage/updated') {
    const total = obj(obj(params.tokenUsage).total).totalTokens
    return { kind: 'event', text: `tokens ${typeof total === 'number' ? total.toLocaleString('en-US') : '?'}` }
  }
  if (method === 'turn/completed') {
    const status = str(obj(params.turn).status)
    return { kind: status === 'completed' ? 'result' : 'error', text: `turn ${status}` }
  }
  if (method === 'error') return { kind: 'error', text: `error: ${str(obj(params.error).message)}` }
  if (method) return { kind: 'event', text: method }
  if ('error' in line) return { kind: 'error', text: `error: ${str(obj(line.error).message)}` }
  return { kind: 'event', text: `response ${JSON.stringify(line.id ?? null)}` }
}

function codexItem(item: Json, done: boolean): LogView {
  switch (item.type) {
    case 'agentMessage':
      return { kind: 'message', text: str(item.text) || 'agent message' }
    case 'reasoning': {
      const summary = list(item.summary).map(str).join('\n')
      return { kind: 'event', text: summary || 'reasoning' }
    }
    case 'userMessage':
      return { kind: 'event', text: 'prompt' }
    case 'commandExecution': {
      const actions = list(item.commandActions).map(obj)
      const command = clip((actions.length === 1 && str(actions[0]?.command)) || str(item.command), 300)
      const exit = item.exitCode
      if (!done || typeof exit !== 'number') return { kind: 'tool', text: `$ ${command}` }
      return { kind: exit === 0 ? 'tool' : 'error', text: `$ ${command} → exit ${String(exit)}` }
    }
    case 'mcpToolCall': {
      const args = obj(item.arguments)
      const name = `${str(item.server)}.${str(item.tool)}`
      const text = Object.keys(args).length > 0 ? `${name} ${clip(JSON.stringify(args), 300)}` : name
      return { kind: done && item.error != null ? 'error' : 'tool', text }
    }
    case 'fileChange':
      return { kind: 'tool', text: `edit ${list(item.changes).map((change) => str(obj(change).path)).join(', ')}` }
    case 'webSearch':
      return { kind: 'tool', text: `search ${str(item.query)}` }
    default:
      return { kind: 'event', text: str(item.type) }
  }
}
