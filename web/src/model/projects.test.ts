import { describe, expect, test } from 'vitest'
import type { IssueRow, StatusInfo } from './issues'
import { milestoneProgress } from './projects'

const GATE_1 = '00000000-0000-0000-0000-000000000001'
const GATE_2 = '00000000-0000-0000-0000-000000000002'

function row(id: string, status: string, milestone: string): IssueRow {
  return {
    issue: {
      id,
      title: `Issue ${id}`,
      body: '',
      status,
      status_name: status,
      created_at: '2026-10-16T10:00:00Z',
      updated_at: '2026-10-16T10:00:00Z',
      labels: [],
      assignee: null,
      milestone: { id: milestone, name: milestone === GATE_1 ? 'Gate 1' : 'Gate 2' },
    },
    run: { state: 'idle' },
  }
}

const statuses: StatusInfo[] = [
  { key: 'backlog', name: 'Backlog', locked_to: [], closed: false },
  { key: 'todo', name: 'Todo', locked_to: [], closed: false },
  { key: 'in_progress', name: 'In Progress', locked_to: [], closed: false },
  { key: 'done', name: 'Done', locked_to: [], closed: true },
  { key: 'canceled', name: 'Canceled', locked_to: [], closed: true },
]

const rows = [
  row('PL-1', 'done', GATE_1),
  row('PL-2', 'todo', GATE_1),
  row('PL-3', 'canceled', GATE_1),
  row('PL-4', 'in_progress', GATE_2),
  row('PL-5', 'backlog', GATE_2),
]

describe('milestoneProgress', () => {
  test('counts done out of the issues not canceled, by status in the workflow order', () => {
    const progress = milestoneProgress(rows, statuses, GATE_1)
    expect({ done: progress.done, total: progress.total }).toEqual({ done: 1, total: 2 })
    expect(progress.canceled).toBe(1)
    expect(progress.byStatus).toEqual([
      { key: 'todo', name: 'Todo', count: 1 },
      { key: 'done', name: 'Done', count: 1 },
    ])
    expect(progress.issues.map((r) => r.issue.id)).toEqual(['PL-2', 'PL-1'])
  })

  test('another milestone counts only its own issues', () => {
    const progress = milestoneProgress(rows, statuses, GATE_2)
    expect({ done: progress.done, total: progress.total, canceled: progress.canceled }).toEqual({
      done: 0,
      total: 2,
      canceled: 0,
    })
    expect(progress.byStatus.map((s) => s.key)).toEqual(['backlog', 'in_progress'])
  })

  test('a milestone with no issues is 0 of 0', () => {
    expect(milestoneProgress(rows, statuses, 'none-of-these')).toEqual({
      done: 0,
      total: 0,
      canceled: 0,
      byStatus: [],
      issues: [],
    })
  })
})
