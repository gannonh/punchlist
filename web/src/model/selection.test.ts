import { describe, expect, test } from 'vitest'
import type { IssueRow, StatusInfo } from './issues'
import type { TransitionList } from './issue-page'
import {
  addRange,
  bulkSummary,
  keepShown,
  labelMark,
  mark,
  moveDisabled,
  moveSummary,
  refusedText,
  toggle,
  withLabel,
} from './selection'

const ORDER = ['PL-1', 'PL-2', 'PL-3', 'PL-4', 'PL-5']

describe('selection', () => {
  test('toggle adds an id, then removes it', () => {
    const one = toggle(new Set(), 'PL-1')
    expect([...one]).toEqual(['PL-1'])
    expect([...toggle(toggle(one, 'PL-2'), 'PL-3')]).toEqual(['PL-1', 'PL-2', 'PL-3'])
    expect([...toggle(one, 'PL-1')]).toEqual([])
  })

  test('a range adds every id between two rows in list order, either way round, keeping the rest', () => {
    expect([...addRange(new Set(['PL-5']), ORDER, 'PL-1', 'PL-3')]).toEqual(['PL-5', 'PL-1', 'PL-2', 'PL-3'])
    expect([...addRange(new Set(), ORDER, 'PL-4', 'PL-2')]).toEqual(['PL-2', 'PL-3', 'PL-4'])
    expect([...addRange(new Set(['PL-2']), ORDER, 'PL-2', 'PL-3')]).toEqual(['PL-2', 'PL-3'])
  })

  test('a range from a row no longer shown changes nothing', () => {
    const selection = new Set(['PL-1'])
    expect(addRange(selection, ORDER, 'PL-9', 'PL-3')).toBe(selection)
  })

  test('a filter that hides a selected row drops it; nothing hidden keeps the same selection', () => {
    const selection = new Set(['PL-1', 'PL-3', 'PL-4'])
    expect([...keepShown(selection, ['PL-1', 'PL-2', 'PL-4'])]).toEqual(['PL-1', 'PL-4'])
    expect(keepShown(selection, ORDER)).toBe(selection)
    expect([...keepShown(selection, [])]).toEqual([])
  })
})

describe('labels', () => {
  test('a label is checked when every issue has it, mixed when some do, unchecked when none', () => {
    expect(labelMark([['idea', 'ui'], ['idea']], 'idea')).toBe(true)
    expect(labelMark([['idea', 'ui'], []], 'idea')).toBe('indeterminate')
    expect(labelMark([['ui'], []], 'idea')).toBe(false)
    expect(mark([])).toBe(false)
  })

  test('adding a label an issue has leaves it there once, and the other labels as they were', () => {
    expect(withLabel(['idea', 'ui'], 'idea', true)).toEqual(['ui', 'idea'])
    expect(withLabel(['ui', 'bug'], 'idea', true)).toEqual(['ui', 'bug', 'idea'])
    expect(withLabel([], 'idea', true)).toEqual(['idea'])
  })

  test('removing a label leaves the others; removing one an issue lacks changes nothing', () => {
    expect(withLabel(['ui', 'idea', 'bug'], 'idea', false)).toEqual(['ui', 'bug'])
    expect(withLabel(['ui'], 'idea', false)).toEqual(['ui'])
  })
})

describe('the toast', () => {
  test('names the change and how many issues it reached', () => {
    expect(bulkSummary('Set Project to Punchlist', 5, [])).toBe('Set Project to Punchlist on 5 issues')
    expect(bulkSummary('Added label idea', 1, [])).toBe('Added label idea on 1 issue')
  })

  test('names each refused issue with its reason', () => {
    const locked = refusedText('PL-12', 'Human Review', { code: 'locked', message: 'human_review is locked' })
    expect(locked).toBe('PL-12 is in Human Review, locked')
    expect(bulkSummary('Set Project to Punchlist', 4, [locked])).toBe('4 updated; PL-12 is in Human Review, locked')
    const other = refusedText('PL-3', 'Todo', { code: 'invalid', message: 'milestone outside the project' })
    expect(bulkSummary('Set Milestone to M1', 0, [locked, other])).toBe(
      '0 updated; PL-12 is in Human Review, locked; PL-3: milestone outside the project',
    )
  })
})

describe('a move of several issues', () => {
  test('the toast says how many moved, then names each refusal', () => {
    expect(moveSummary('Todo', [{ id: 'PL-1' }, { id: 'PL-2' }, { id: 'PL-3' }])).toBe('Moved 3 to Todo')
    expect(
      moveSummary('Start', [
        { id: 'PL-11' },
        { id: 'PL-12', error: { code: 'gate_failed', gate: 'ci_green', reason: 'no check run', message: 'todo → start needs gate ci_green: no check run' } },
        { id: 'PL-13' },
        { id: 'PL-14', error: { code: 'role_not_allowed', message: 'only a person makes todo → start, not an agent' } },
      ]),
    ).toBe('Moved 2 to Start; PL-12 refused: gate ci_green failed; PL-14 refused: only a person makes todo → start, not an agent')
    expect(moveSummary('Todo', [{ id: 'PL-4', error: { code: 'locked', message: 'human_review is locked' } }])).toBe(
      'Moved 0 to Todo; PL-4 refused: human_review is locked',
    )
  })

  const todo: StatusInfo = { key: 'todo', name: 'Todo', locked_to: [], closed: false }
  const start: StatusInfo = { key: 'start', name: 'Start', locked_to: [], closed: false }
  const done: StatusInfo = { key: 'done', name: 'Done', locked_to: [], closed: false }
  const issue = (id: string, status: StatusInfo) =>
    ({ id, title: id, body: '', status: status.key, status_name: status.name, created_at: '', updated_at: '' }) as IssueRow['issue']
  const moves = (from: StatusInfo, verdict: TransitionList['transitions'][number]['verdict']): TransitionList => ({
    status: from,
    viewer_locked: false,
    transitions: [{ to: start, by: ['person'], verdict }],
  })
  const roleRefused = moves(todo, { kind: 'refused', code: 'role_not_allowed', message: 'only a person makes todo → start, not an agent' })

  test('a move no selected issue can make is disabled with each reason once', () => {
    expect(moveDisabled(start, [issue('PL-1', todo), issue('PL-2', todo)], [roleRefused, roleRefused])).toBe(
      'only a person makes todo → start, not an agent',
    )
    expect(moveDisabled(done, [issue('PL-1', todo), issue('PL-2', done)], [roleRefused, undefined])).toBe(
      'Todo → Done cannot be requested; already in Done',
    )
  })

  test('a move one selected issue can make, or may once its moves load, is enabled; a blocked one too', () => {
    const allowed = moves(todo, { kind: 'allowed' })
    const blocked = moves(todo, { kind: 'blocked', gate: 'ci_green', passed: false, reason: 'no check run' })
    expect(moveDisabled(start, [issue('PL-1', todo), issue('PL-2', todo)], [roleRefused, allowed])).toBeNull()
    expect(moveDisabled(start, [issue('PL-1', todo)], [blocked])).toBeNull()
    expect(moveDisabled(start, [issue('PL-1', todo), issue('PL-2', todo)], [roleRefused, undefined])).toBeNull()
  })
})
