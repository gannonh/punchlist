import { describe, expect, it } from 'vitest'
import { ageText, canRunAgain, countText, ordered, refusalParts, runLines, type InboxItem, type RefusalItem, type RunItem } from './inbox'

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

const run = (last_outcome: RunItem['last_outcome'], reason: string | null, limit_reached: boolean): RunItem => ({
  kind: 'run',
  issue: { id: 'PL-7', title: 'Issue PL-7', status: 'in_progress', status_name: 'In Progress' },
  agent: 'claude-code',
  runs: limit_reached ? 3 : 1,
  last_outcome,
  reason,
  limit_reached,
  since: '2026-10-24T11:00:00Z',
  since_seq: 9,
  unread: true,
})

describe('runLines', () => {
  it('says the entry ran out of runs, and why the last one failed', () => {
    expect(runLines(run('succeeded', null, true))).toEqual(['3 runs in In Progress ended without moving it'])
    expect(runLines(run('failed', 'agent crashed', true))).toEqual([
      '3 runs in In Progress ended without moving it',
      'Run failed: agent crashed',
    ])
    expect(runLines(run('failed', null, true))).toEqual(['3 runs in In Progress ended without moving it', 'Run failed'])
  })

  it('says a run failed before the limit', () => {
    expect(runLines(run('failed', 'lease expired', false))).toEqual(['Run failed: lease expired'])
  })
})

const refusal = (reason: string): RefusalItem => ({
  kind: 'refusal',
  issue: { id: 'PL-2', title: 'Issue PL-2', status: 'agent_review', status_name: 'Agent Review' },
  agent: { id: '00000000-0000-0000-0000-000000000001', name: 'Reviewer', role: 'agent', kind: 'codex' },
  from: 'agent_review',
  from_name: 'Agent Review',
  to: 'human_review',
  to_name: 'Human Review',
  gate: 'ci_green',
  reason,
  since: '2026-10-24T11:30:00Z',
  since_seq: 12,
  unread: true,
})

describe('refusalParts', () => {
  it('names the agent, the move by status names, the gate and the reason', () => {
    expect(refusalParts(refusal('1 of 6 checks failed on 3f2a1c4: test'))).toEqual({
      agent: 'Reviewer (codex)',
      move: 'Agent Review → Human Review',
      gate: 'ci_green',
      reason: '1 of 6 checks failed on 3f2a1c4: test',
    })
  })

  it('keeps a reason with markup as the characters it has', () => {
    expect(refusalParts(refusal('check <b>x</b> failed')).reason).toBe('check <b>x</b> failed')
  })
})

describe('canRunAgain', () => {
  it('is true for a run item whose entry used its last run, and for nothing else', () => {
    expect(
      [
        run('succeeded', null, true),
        run('failed', 'agent crashed', true),
        run('failed', 'lease expired', false),
        item('PL-1', 'todo', 'Todo', '2026-10-24T11:56:00Z', 5),
        refusal('1 of 6 checks failed on 3f2a1c4: test'),
      ].map(canRunAgain),
    ).toEqual([true, true, false, false, false])
  })
})
