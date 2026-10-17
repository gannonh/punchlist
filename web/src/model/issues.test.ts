import { describe, expect, test } from 'vitest'
import {
  applyMove,
  applyStream,
  applyWrite,
  filterRows,
  fromList,
  groupByStatus,
  refusalText,
  replaceRow,
  revertEdit,
  rowView,
  type IssueRow,
  type StatusInfo,
} from './issues'

const now = new Date('2026-10-16T12:00:00Z')
const RUNNING = { state: 'running', agent: 'claude-code', since: '2026-10-16T11:00:00Z' } as const

function row(id: string, overrides: Partial<IssueRow['issue']> = {}, rest: Partial<IssueRow> = {}): IssueRow {
  return {
    issue: {
      id,
      title: `Issue ${id}`,
      body: '',
      status: 'in_progress',
      status_name: 'In Progress',
      created_at: '2026-10-16T10:00:00Z',
      updated_at: '2026-10-16T10:00:00Z',
      labels: [],
      assignee: null,
      ...overrides,
    },
    run: { state: 'idle' },
    ...rest,
  }
}

const statuses: StatusInfo[] = [
  { key: 'todo', name: 'Todo', locked_to: [], closed: false },
  { key: 'in_progress', name: 'In Progress', locked_to: [], closed: false },
  { key: 'human_review', name: 'Human Review', locked_to: ['agent', 'runner'], closed: false },
]

describe('rowView', () => {
  test('a gate summary reads "3 of 4 gates pass"', () => {
    const view = rowView(
      row(
        'PL-1',
        {},
        { gates: { to: 'human_review', to_name: 'Human Review', passed: 3, total: 4, first_failed: 'proof_attached' } },
      ),
      now,
    )
    expect(view.gates.text).toBe('3 of 4 gates pass')
    expect(view.gates.title).toBe('to Human Review; first failing: proof_attached')
  })

  test('no gated transition reads "no gates"', () => {
    expect(rowView(row('PL-1'), now).gates.text).toBe('no gates')
  })

  test('an agent shows its kind, a person their name, and nobody "Unassigned"', () => {
    const agent = rowView(
      row('PL-1', { assignee: { id: 'a', name: 'claude-code (Ana)', role: 'agent', kind: 'claude-code' } }),
      now,
    )
    const person = rowView(row('PL-2', { assignee: { id: 'p', name: 'Ana Lee', role: 'person', kind: null } }), now)
    const nobody = rowView(row('PL-3'), now)
    expect(agent.assignee).toEqual({ kind: 'agent', label: 'Claude Code', name: 'claude-code (Ana)' })
    expect(person.assignee).toEqual({ kind: 'person', label: 'Ana Lee', initials: 'AL' })
    expect(nobody.assignee).toEqual({ kind: 'unassigned', label: 'Unassigned' })
  })

  test('run and pull request labels', () => {
    const view = rowView(
      row(
        'PL-1',
        {},
        {
          run: { state: 'running', agent: 'codex', since: '2026-10-16T11:56:00Z' },
          pull_request: { number: 12, state: 'open', checks: 'failing', url: null },
        },
      ),
      now,
    )
    expect(view.run.label).toBe('Running · 4m')
    expect(view.pullRequest.label).toBe('#12 open')
    expect(view.pullRequest.title).toBe('PR #12 open, checks failing')
    expect(
      rowView(row('PL-2', {}, { run: { state: 'failed', agent: 'codex', at: now.toISOString(), reason: 'lost' } }), now)
        .run,
    ).toEqual({
      state: 'failed',
      label: 'Run failed',
      detail: 'lost',
    })
    expect(rowView(row('PL-3'), now).pullRequest.label).toBe('no PR')
  })
})

describe('refusalText', () => {
  test('role_not_allowed shows its code and the rule', () => {
    expect(
      refusalText({
        code: 'role_not_allowed',
        message: 'only an agent makes in_progress → agent_review, not a person',
      }),
    ).toBe('Refused (role_not_allowed): only an agent makes in_progress → agent_review, not a person')
  })

  test('gate_failed shows the gate and its reason', () => {
    expect(
      refusalText({
        code: 'gate_failed',
        message: 'in_progress → agent_review needs gate pr_ready: the pull request is a draft',
        gate: 'pr_ready',
        reason: 'the pull request is a draft',
      }),
    ).toBe('Refused (gate_failed): gate pr_ready failed: the pull request is a draft')
  })
})

describe('filters and groups', () => {
  const rows = [
    row('PL-1', { title: 'Fix the login', status: 'todo', status_name: 'Todo' }),
    row('PL-2', { assignee: { id: 'a', name: 'codex (Ana)', role: 'agent', kind: 'codex' } }),
    row('PL-3', { assignee: { id: 'p', name: 'Ana Lee', role: 'person', kind: null }, labels: ['web'] }),
  ]

  test('groups follow the workflow order, empty ones included', () => {
    expect(groupByStatus(rows, statuses).map((g) => [g.status.key, g.rows.map((r) => r.issue.id)])).toEqual([
      ['todo', ['PL-1']],
      ['in_progress', ['PL-2', 'PL-3']],
      ['human_review', []],
    ])
  })

  test('text and assignee filters', () => {
    const ids = (who: Parameters<typeof filterRows>[1]) => filterRows(rows, who).map((r) => r.issue.id)
    expect(ids({ text: 'login', who: '' })).toEqual(['PL-1'])
    expect(ids({ text: 'WEB', who: '' })).toEqual(['PL-3'])
    expect(ids({ text: '', who: 'agents' })).toEqual(['PL-2'])
    expect(ids({ text: '', who: 'agent:codex' })).toEqual(['PL-2'])
    expect(ids({ text: '', who: 'agent:claude-code' })).toEqual([])
    expect(ids({ text: '', who: 'people' })).toEqual(['PL-3'])
    expect(ids({ text: '', who: 'none' })).toEqual(['PL-1'])
  })
})

describe('the cache', () => {
  const event = (seq: number, issue_id: string) => ({
    seq,
    issue_id,
    actor: { id: 'p', name: 'Ana', role: 'person' as const },
    created_at: now.toISOString(),
    detail: { kind: 'created' as const },
  })

  test('a stream message at or below the last sequence is ignored', () => {
    const cache = fromList({ issues: [row('PL-1')], last_event_seq: 5 })
    const stale = applyStream(cache, { event: event(5, 'PL-1'), row: row('PL-1', { title: 'stale' }) })
    expect(stale.rows[0]?.issue.title).toBe('Issue PL-1')
    const fresh = applyStream(cache, { event: event(6, 'PL-2'), row: row('PL-2') })
    expect(fresh.rows.map((r) => r.issue.id)).toEqual(['PL-1', 'PL-2'])
    expect(fresh.lastSeq).toBe(6)
  })

  test('a run message and a pull request message replace the row they name', () => {
    const cache = fromList({ issues: [row('PL-1'), row('PL-2')], last_event_seq: 4 })
    const running = { state: 'running' as const, agent: 'claude-code', since: '2026-10-16T11:59:30Z' }
    const claimed = applyStream(cache, {
      event: { ...event(5, 'PL-1'), detail: { kind: 'run' } },
      row: row('PL-1', {}, { run: running }),
    })
    const ready = applyStream(claimed, {
      event: { ...event(6, 'PL-1'), detail: { kind: 'pull_request' } },
      row: row('PL-1', {}, { run: running, pull_request: { number: 12, state: 'open', checks: 'pending', url: null } }),
    })
    const views = ready.rows.map((r) => rowView(r, now)).map((v) => [v.id, v.run.label, v.pullRequest.title])
    expect(views).toEqual([
      ['PL-1', 'Running · 30s', 'PR #12 open, checks pending'],
      ['PL-2', 'Idle', 'no pull request'],
    ])
    expect(ready.lastSeq).toBe(6)
  })

  test("a write's answer older than the row the stream gave is dropped", () => {
    const cache = applyStream(fromList({ issues: [row('PL-1')], last_event_seq: 1 }), {
      event: event(3, 'PL-1'),
      row: row('PL-1', { title: 'from the stream' }),
    })
    expect(applyWrite(cache, { seq: 2, row: row('PL-1', { title: 'older' }) }).rows[0]?.issue.title).toBe(
      'from the stream',
    )
    expect(applyWrite(cache, { seq: 4, row: row('PL-1', { title: 'newer' }) }).rows[0]?.issue.title).toBe('newer')
  })

  test("a transition's answer older than the row the stream gave is dropped", () => {
    const cache = applyStream(fromList({ issues: [row('PL-1', { status: 'todo' })], last_event_seq: 1 }), {
      event: event(3, 'PL-1'),
      row: row('PL-1', { status: 'in_progress', status_name: 'In Progress' }),
    })
    const moved = (seq: number) => ({
      issue: row('PL-1', { status: 'start', status_name: 'Start' }).issue,
      event: event(seq, 'PL-1'),
    })
    expect(applyMove(cache, moved(2)).rows[0]?.issue.status).toBe('in_progress')
    expect(applyMove(cache, moved(3)).rows[0]?.issue.status).toBe('in_progress')
    expect(applyMove(cache, moved(4)).rows[0]?.issue.status).toBe('start')
  })

  test('a refused edit puts back only its own fields, where nothing changed them since', () => {
    const before = row('PL-1', { title: 'Old', labels: ['a'] })
    const edited = { ...before.issue, title: 'Mine', labels: ['b'] }
    const since = replaceRow(fromList({ issues: [before], last_event_seq: 4 }), 'PL-1', (r) => ({
      ...r,
      issue: { ...r.issue, title: 'Mine', labels: ['c'], status: 'todo', status_name: 'Todo' },
    }))
    const reverted = revertEdit(since, edited, ['title', 'labels'], 4)
    expect(reverted.rows[0]?.issue).toMatchObject({ title: 'Old', labels: ['c'], status: 'todo' })
  })

  test('a refused edit puts back the parent, project and milestone it changed', () => {
    const punchlist = { id: 'p1', name: 'Punchlist' }
    const before = row('PL-2', { parent: null, project: punchlist, milestone: { id: 'm1', name: 'Gate 0' } })
    const edited = { ...before.issue, parent: 'PL-1', project: { id: 'p2', name: 'Groundwork' }, milestone: null }
    const cache = replaceRow(fromList({ issues: [before], last_event_seq: 4 }), 'PL-2', (r) => ({
      ...r,
      issue: edited,
    }))
    const reverted = revertEdit(cache, edited, ['parent', 'project', 'milestone'], 4)
    expect(reverted.rows[0]?.issue).toMatchObject({
      parent: null,
      project: punchlist,
      milestone: { id: 'm1', name: 'Gate 0' },
    })
  })

  test('a refused edit leaves a row the server sent since, even one with the same value', () => {
    const before = row('PL-1', { title: 'Old' })
    const edited = { ...before.issue, title: 'Mine' }
    const cache = applyStream(fromList({ issues: [row('PL-1', { title: 'Old' })], last_event_seq: 4 }), {
      event: event(5, 'PL-1'),
      row: row('PL-1', { title: 'Mine' }),
    })
    expect(revertEdit(cache, edited, ['title'], 4).rows[0]?.issue.title).toBe('Mine')
  })

  test('a row event for a run, arriving while an edit is pending, is what a refused edit shows', () => {
    const punchlist = { id: 'p1', name: 'Punchlist' }
    const before = row('PL-1', { title: 'Old', parent: null, project: punchlist })
    const seq = 4
    const edited = { ...before.issue, title: 'Mine', parent: 'PL-9' }
    const pending = replaceRow(fromList({ issues: [before], last_event_seq: seq }), 'PL-1', (r) => ({ ...r, issue: edited }))
    // A `run` event: its row has the run's new state and a title another editor set; it is the server's row.
    const streamed = applyStream(pending, {
      event: event(5, 'PL-1'),
      row: row('PL-1', { title: 'Theirs', parent: null, project: punchlist }, { run: RUNNING }),
    })
    expect(streamed.rows[0]?.issue).toMatchObject({ title: 'Theirs', parent: null })
    const reverted = revertEdit(streamed, edited, ['title', 'parent'], seq)
    expect(reverted.rows[0]?.issue).toMatchObject({ title: 'Theirs', parent: null, project: punchlist })
    expect(reverted.rows[0]?.run).toEqual(RUNNING)
    expect(reverted.confirmed['PL-1']?.title).toBe('Theirs')
  })

  // Two edits to one field made while the first is in flight, settled oldest first, as queries.ts runs them.
  function stackedParentEdits() {
    let cache = fromList({ issues: [row('PL-2', { parent: null })], last_event_seq: 4 })
    const edit = (parent: string) => {
      const edited = { ...row('PL-2', { ...cache.rows[0]?.issue }).issue, parent }
      cache = replaceRow(cache, 'PL-2', (r) => ({ ...r, issue: edited }))
      return { edited, seq: cache.rowSeq['PL-2'] ?? -1 }
    }
    return { first: edit('PL-3'), second: edit('PL-1'), cache: () => cache, set: (c: typeof cache) => (cache = c) }
  }

  test('two stacked edits to one field, both refused oldest first, show the server value', () => {
    const s = stackedParentEdits()
    for (const e of [s.first, s.second]) s.set(revertEdit(s.cache(), e.edited, ['parent'], e.seq))
    expect(s.cache().rows[0]?.issue.parent).toBe(null)
  })

  test('two stacked edits to one field, the first refused and the second accepted, show the second', () => {
    const s = stackedParentEdits()
    s.set(revertEdit(s.cache(), s.first.edited, ['parent'], s.first.seq))
    expect(s.cache().rows[0]?.issue.parent).toBe('PL-1')
    s.set(applyWrite(s.cache(), { seq: 6, row: row('PL-2', { parent: 'PL-1' }) }))
    expect(s.cache().rows[0]?.issue.parent).toBe('PL-1')
  })
})
