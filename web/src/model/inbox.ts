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
