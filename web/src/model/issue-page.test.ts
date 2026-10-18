import { describe, expect, test } from 'vitest'
import {
  actorMark,
  eventText,
  feedEntries,
  gateSummary,
  moveView,
  specBlocks,
  tokens,
  duration,
  type GateChecklist,
  type IssueEvent,
  type TransitionOption,
} from './issue-page'

const ciGreen = { gate: 'ci_green', passed: false, reason: '1 of 1 checks failed on 0000000', outside: ['`check` (failure)'] }
const checklist: GateChecklist = {
  from: 'agent_review',
  from_name: 'Agent Review',
  to: 'human_review',
  to_name: 'Human Review',
  gates: [
    ciGreen,
    { gate: 'mergeable', passed: true, reason: 'pull request #7 is mergeable' },
    { gate: 'no_open_threads', passed: true, reason: 'no review thread is open' },
    { gate: 'proof_attached', passed: false, reason: 'no proof is attached to PL-7 for attempt 1' },
  ],
}

let seq = 0
function event(role: IssueEvent['actor']['role'], name: string, detail: IssueEvent['detail'], kind?: string) {
  seq += 1
  return {
    seq,
    issue_id: 'PL-7',
    actor: { id: '00000000-0000-0000-0000-000000000001', name, role, ...(kind && { kind }) },
    created_at: '2026-10-18T10:00:00Z',
    detail,
  } satisfies IssueEvent
}

const created = event('person', 'Gannon Hall', { kind: 'created' })
const comment = event('agent', 'Codex', { kind: 'comment', body: 'Opened #7.\n\n(agent)' }, 'codex')
const moved = event('runner', 'sartre', {
  kind: 'transition',
  from: 'start',
  from_name: 'Start',
  to: 'in_progress',
  to_name: 'In Progress',
  workflow_version: 'sha256:1',
})
const refused = event('github', 'gannonh', {
  kind: 'refusal',
  from: 'merging',
  from_name: 'Merging',
  to: 'done',
  to_name: 'Done',
  gate: 'pr_names_issue',
  reason: 'pull request #7 names no issue',
  workflow_version: 'sha256:1',
})
const edited = event('person', 'Gannon Hall', { kind: 'edited', fields: ['title', 'labels', 'assignee'] })
const timeline = [created, comment, moved, refused, edited]

describe('gateSummary', () => {
  test('counts the passing gates and names the first that fails', () => {
    expect(gateSummary(checklist)).toEqual({
      text: '2 of 4 gates pass',
      passed: 2,
      total: 4,
      toName: 'Human Review',
      firstFailed: 'ci_green',
    })
  })

  test('a status with no gated move has no gates', () => {
    expect(gateSummary(null)).toEqual({
      text: 'No gates on the next move',
      passed: 0,
      total: 0,
      toName: null,
      firstFailed: null,
    })
  })
})

describe('actorMark', () => {
  test('marks people, agents, runners and GitHub, and a refusal as the server', () => {
    expect(timeline.map(actorMark)).toEqual([
      { mark: 'person', name: 'Gannon Hall', chip: null, initials: 'GH' },
      { mark: 'agent', name: 'Codex', chip: 'Codex', initials: '' },
      { mark: 'runner', name: 'sartre', chip: 'runner', initials: '' },
      { mark: 'server', name: 'Punchlist', chip: 'server', initials: 'PL' },
      { mark: 'person', name: 'Gannon Hall', chip: null, initials: 'GH' },
    ])
  })
})

describe('feedEntries', () => {
  const seqs = (feed: Parameters<typeof feedEntries>[1]) => feedEntries(timeline, feed).map((e) => e.seq)

  test('All keeps every entry, Comments the comments, Transitions the moves and refusals', () => {
    expect(seqs('all')).toEqual([created.seq, comment.seq, moved.seq, refused.seq, edited.seq])
    expect(seqs('comments')).toEqual([comment.seq])
    expect(seqs('transitions')).toEqual([moved.seq, refused.seq])
  })

  test('entries read as sentences', () => {
    expect(timeline.map(eventText)).toEqual([
      'created the issue',
      'Opened #7.\n\n(agent)',
      'moved Start → In Progress',
      "refused gannonh's move to Done: gate pr_names_issue failed: pull request #7 names no issue",
      'edited the title, labels and assignee',
    ])
  })
})

describe('moveView', () => {
  const human: TransitionOption['to'] = {
    key: 'human_review',
    name: 'Human Review',
    locked_to: ['agent', 'runner'],
    closed: false,
  }
  const option = (verdict: TransitionOption['verdict']): TransitionOption => ({ to: human, by: ['agent'], verdict })

  test('a blocked move names its gate, reason and checks', () => {
    const blocked = option({ kind: 'blocked', ...ciGreen })
    expect(moveView(blocked, false, checklist)).toEqual({
      to: 'human_review',
      toName: 'Human Review',
      peopleOnly: true,
      by: 'by agent',
      state: 'blocked',
      text: '1 of 1 checks failed on 0000000',
      gate: 'ci_green',
      names: ['`check` (failure)'],
      disabled: true,
    })
  })

  test('a move the role may not make is hidden; a locked one is disabled with its rule', () => {
    expect(moveView(option({ kind: 'refused', code: 'role_not_allowed', message: 'only an agent' }), false, null)).toBe(
      null,
    )
    const locked = moveView(
      option({ kind: 'refused', code: 'locked', message: 'human_review is locked: an agent cannot act on an issue in it' }),
      true,
      null,
    )
    expect([locked?.state, locked?.text, locked?.disabled]).toEqual([
      'locked',
      'human_review is locked: an agent cannot act on an issue in it',
      true,
    ])
  })

  test("another role's move shows its failing gate to a person, and is hidden when nothing fails", () => {
    const notMine = option({ kind: 'refused', code: 'role_not_allowed', message: 'only an agent' })
    expect(moveView(notMine, false, checklist)).toEqual({
      to: 'human_review',
      toName: 'Human Review',
      peopleOnly: true,
      by: 'by agent',
      state: 'blocked',
      text: '1 of 1 checks failed on 0000000',
      gate: 'ci_green',
      names: ['`check` (failure)'],
      disabled: true,
    })
    const passing = { ...checklist, gates: checklist.gates.map((gate) => ({ ...gate, passed: true })) }
    expect(moveView(notMine, false, passing)).toBe(null)
  })

  test('an allowed move says how many gates pass', () => {
    expect(moveView(option({ kind: 'allowed' }), false, checklist)?.text).toBe('All 4 gates pass.')
    expect(moveView(option({ kind: 'allowed' }), false, null)?.text).toBe('No gates on this move.')
  })
})

describe('specBlocks and run facts', () => {
  test('a body splits into headings, items, criteria and paragraphs', () => {
    expect(specBlocks('Fix the thing\nproperly.\n\n## Acceptance criteria\n- [x] It works\n- [ ] It is fast\n- Note')).toEqual([
      { kind: 'text', text: 'Fix the thing properly.' },
      { kind: 'heading', text: 'Acceptance criteria' },
      { kind: 'check', text: 'It works', done: true },
      { kind: 'check', text: 'It is fast', done: false },
      { kind: 'item', text: 'Note' },
    ])
  })

  test('durations and tokens read short', () => {
    expect([duration(38_000), duration(252_000), duration(3_780_000), duration(null)]).toEqual([
      '38s',
      '4m 12s',
      '1h 3m',
      '—',
    ])
    expect([tokens(12_400, 3_100), tokens(800, 20), tokens(null, 5)]).toEqual(['12.4k in · 3.1k out', '800 in · 20 out', '—'])
  })
})
