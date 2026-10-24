import { describe, expect, it } from 'vitest'
import { ageText, countText, ordered, type InboxItem } from './inbox'

const item = (id: string, status: string, status_name: string, since: string, since_seq: number): InboxItem => ({
  kind: 'waiting',
  issue: { id, title: `Issue ${id}`, status, status_name },
  since,
  since_seq,
  unread: true,
})

const NOW = new Date('2026-10-24T12:00:00Z')
const ITEMS = [
  item('PL-3', 'human_review', 'Human Review', '2026-10-24T09:00:00Z', 3),
  item('PL-1', 'todo', 'Todo', '2026-10-24T11:56:00Z', 5),
  item('PL-4', 'merging', 'Merging', '2026-10-21T12:00:00Z', 4),
]

describe('ordered', () => {
  it('puts the newest event first', () => {
    expect(ordered(ITEMS).map((i) => i.issue.id)).toEqual(['PL-1', 'PL-4', 'PL-3'])
  })
})

describe('ageText', () => {
  it('says how long each item has waited', () => {
    expect(ordered(ITEMS).map((i) => ageText(i, NOW))).toEqual(['4m ago', '3d ago', '3h ago'])
  })
})

describe('countText', () => {
  it('shows the unread count, at most 99+', () => {
    expect([0, 3, 99, 100].map(countText)).toEqual(['0', '3', '99', '99+'])
  })
})
