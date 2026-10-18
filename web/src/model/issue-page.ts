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
export type GateChecklist = Schemas['GateChecklist']
export type GateResult = Schemas['GateResult']
export type TransitionList = Schemas['TransitionList']
export type TransitionOption = Schemas['TransitionOption']
export type LogLine = Schemas['LogLine']

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
 * would have made, so the entry is marked as the server's and names GitHub in its text.
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
    return { ...base, state: 'blocked', text: verdict.reason, gate: verdict.gate, names, disabled: true }
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
