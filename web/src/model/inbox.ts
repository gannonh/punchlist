// What the inbox shows, as pure functions over the API's types. No React here.
import type { components } from '@/api/schema'
import { age } from '@/model/issues'

export type InboxList = components['schemas']['InboxList']
export type InboxItem = components['schemas']['InboxItem']

/** The unread count as the header shows it: at most `99+`. */
export const countText = (unread: number): string => (unread > 99 ? '99+' : String(Math.max(0, unread)))

/** How long the item has waited, as `4m ago`. */
export const ageText = (item: InboxItem, now: Date): string => `${age(item.since, now)} ago`

/** Newest first: the item whose event came last leads. */
export const ordered = (items: readonly InboxItem[]): InboxItem[] =>
  [...items].sort((a, b) => b.since_seq - a.since_seq || a.issue.id.localeCompare(b.issue.id))

export type RunItem = Extract<InboxItem, { kind: 'run' }>

/**
 * What a run item says, a line each: the server's sentence when the entry's runs are used up, then
 * `Run failed` with its reason when the run failed. The reason is text from outside the team.
 */
export const runLines = (item: RunItem): string[] => [
  ...(item.limit_reached ? [`${String(item.runs)} runs in ${item.issue.status_name} ended without moving it`] : []),
  ...(item.last_outcome === 'failed' ? [item.reason ? `Run failed: ${item.reason}` : 'Run failed'] : []),
]

export type RefusalItem = Extract<InboxItem, { kind: 'refusal' }>

/**
 * What a refusal item says: the agent that asked, as `Reviewer (codex)`, the move by status names, the gate and the
 * reason. The reason can name checks GitHub's users wrote: text from outside the team, rendered as text.
 */
export const refusalParts = (item: RefusalItem) => ({
  agent: item.agent.kind ? `${item.agent.name} (${item.agent.kind})` : item.agent.name,
  move: `${item.from_name} → ${item.to_name}`,
  gate: item.gate,
  reason: item.reason,
})
