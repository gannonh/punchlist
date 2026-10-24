import { describe, expect, test } from 'vitest'
import {
  actorMark,
  eventText,
  feedEntries,
  gateSummary,
  logView,
  mediaStep,
  modelView,
  moveView,
  plainPullRequestUrl,
  plainThreadUrl,
  proofType,
  threadView,
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

  test('a blocked move names its gate, reason and checks, and can still be requested', () => {
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
      disabled: false,
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

  test('a pull request links only to its plain GitHub URL', () => {
    expect([
      plainPullRequestUrl(7, 'https://github.com/gannonh/punchlist/pull/7'),
      plainPullRequestUrl(7, 'https://github.com/gannonh/punchlist/pull/8'),
      plainPullRequestUrl(7, 'javascript:alert(1)//https://github.com/a/b/pull/7'),
      plainPullRequestUrl(7, 'https://github.com.evil.example/a/b/pull/7'),
      plainPullRequestUrl(7, 'https://github.com/../b/pull/7'),
    ]).toEqual(['https://github.com/gannonh/punchlist/pull/7', null, null, null, null])
  })

  test('a review thread links only to its plain GitHub discussion URL', () => {
    const url = 'https://github.com/gannonh/punchlist/pull/12#discussion_r1700001'
    expect([
      plainThreadUrl(12, url),
      plainThreadUrl(13, url),
      plainThreadUrl(12, 'javascript:alert(1)//https://github.com/a/b/pull/12#discussion_r1'),
      plainThreadUrl(12, 'https://github.com.evil.example/a/b/pull/12#discussion_r1'),
      plainThreadUrl(12, 'https://github.com/a/b/pull/12#discussion_rabc'),
      plainThreadUrl(12, 'https://github.com/a/b/pull/12'),
      plainThreadUrl(12, null),
    ]).toEqual([url, null, null, null, null, null, null])
  })

  test('a review thread shows its place, how many comments it has and which are cut', () => {
    const comment = { author: 'octocat', body: 'Should this return an error?' }
    const thread = {
      id: 'PRRT_1',
      pull_request: 12,
      path: 'src/lib.rs',
      line: 41,
      comments: [{ ...comment, cut: true }, ...Array.from({ length: 9 }, () => comment)],
      comments_total: 14,
      url: 'https://github.com/gannonh/punchlist/pull/12#discussion_r1700001',
    }
    const view = threadView(thread)
    expect([view.place, view.note, view.unread, view.url, view.comments[0], view.comments[1]]).toEqual([
      'src/lib.rs:41',
      'last 10 of 14 comments',
      false,
      'https://github.com/gannonh/punchlist/pull/12#discussion_r1700001',
      { ...comment, cut: true },
      { ...comment, cut: false },
    ])
    expect(threadView({ ...thread, comments_total: 10 }).note).toBe(null)
    expect(threadView({ ...thread, path: null }).place).toBe('no file')
    expect(threadView({ ...thread, line: null }).place).toBe('src/lib.rs')
    expect(threadView({ ...thread, comments: [], comments_total: 0, url: 'javascript:alert(1)' })).toEqual({
      place: 'src/lib.rs:41',
      note: null,
      comments: [],
      unread: true,
      url: null,
    })
  })

  test("proof takes the server's type from its extension", () => {
    expect([proofType('run.LOG'), proofType('shot.jpg'), proofType('page.html'), proofType('png')]).toEqual([
      'text/plain',
      'image/jpeg',
      null,
      null,
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

describe('logView', () => {
  const claude = (message: unknown) => JSON.stringify({ type: 'assistant', message, session_id: 's' })

  test('a Claude Code assistant line shows its text whole', () => {
    const line = claude({ role: 'assistant', content: [{ type: 'text', text: 'I will read the issue, then open the pull request.' }] })
    expect(logView(line)).toEqual({ kind: 'message', text: 'I will read the issue, then open the pull request.' })
  })

  test('a Claude Code tool call shows the tool and its file or command', () => {
    expect(logView(claude({ content: [{ type: 'tool_use', id: 't1', name: 'Read', input: { file_path: '/w/src/main.rs' } }] }))).toEqual({
      kind: 'tool',
      text: 'Read /w/src/main.rs',
    })
    const bash = claude({ content: [{ type: 'tool_use', name: 'Bash', input: { command: 'cargo test -p core\necho done', description: 'Run tests' } }] })
    expect(logView(bash)).toEqual({ kind: 'tool', text: 'Bash cargo test -p core …' })
    expect(logView(claude({ content: [{ type: 'tool_use', name: 'mcp__punchlist__get_issue', input: {} }] }))).toEqual({
      kind: 'tool',
      text: 'mcp__punchlist__get_issue',
    })
  })

  test('a Claude Code tool result, session start and finish', () => {
    const result = JSON.stringify({
      type: 'user',
      message: { content: [{ type: 'tool_result', tool_use_id: 't1', content: [{ type: 'text', text: 'fn main() {}\n// more' }] }] },
    })
    expect(logView(result)).toEqual({ kind: 'result', text: 'result: fn main() {} …' })
    const failed = JSON.stringify({ type: 'user', message: { content: [{ type: 'tool_result', is_error: true, content: 'No such file' }] } })
    expect(logView(failed)).toEqual({ kind: 'error', text: 'error: No such file' })
    expect(logView(JSON.stringify({ type: 'system', subtype: 'init', model: 'claude-haiku-5-5' }))).toEqual({
      kind: 'event',
      text: 'session started, model claude-haiku-5-5',
    })
    expect(logView(JSON.stringify({ type: 'result', subtype: 'success', is_error: false, result: 'Opened #12.' }))).toEqual({
      kind: 'result',
      text: 'finished success: Opened #12.',
    })
  })

  test('a Codex item line shows its summary', () => {
    const item = (method: string, item: unknown) => JSON.stringify({ method, params: { item, threadId: 't' } })
    expect(logView(item('item/completed', { type: 'agentMessage', id: 'm', text: 'PR #99 has one failing check.', phase: 'commentary' }))).toEqual({
      kind: 'message',
      text: 'PR #99 has one failing check.',
    })
    const command = {
      type: 'commandExecution',
      command: "/usr/bin/bash -lc 'gh pr checks 99'",
      commandActions: [{ type: 'unknown', command: 'gh pr checks 99' }],
      exitCode: null,
    }
    expect(logView(item('item/started', command))).toEqual({ kind: 'tool', text: '$ gh pr checks 99' })
    expect(logView(item('item/completed', { ...command, exitCode: 1 }))).toEqual({ kind: 'error', text: '$ gh pr checks 99 → exit 1' })
    expect(logView(item('item/completed', { type: 'mcpToolCall', server: 'punchlist', tool: 'request_transition', arguments: { to: 'human_review' }, error: null }))).toEqual({
      kind: 'tool',
      text: 'punchlist.request_transition {"to":"human_review"}',
    })
    const usage = { method: 'thread/tokenUsage/updated', params: { tokenUsage: { total: { totalTokens: 21810 } } } }
    expect(logView(JSON.stringify(usage))).toEqual({ kind: 'event', text: 'tokens 21,810' })
  })

  test('a line that is not a JSON object is shown as written', () => {
    expect(logView('stderr: warning: unused import')).toEqual({ kind: 'plain', text: 'stderr: warning: unused import' })
    expect(logView('{"type": "assistant"')).toEqual({ kind: 'plain', text: '{"type": "assistant"' })
    expect(logView('42')).toEqual({ kind: 'plain', text: '42' })
  })
})

describe('mediaStep', () => {
  const proof = [
    { id: 'a', kind: 'image' },
    { id: 'r', kind: 'report' },
    { id: 'v', kind: 'video' },
    { id: 'b', kind: 'image' },
  ] as const

  test('steps through images and videos in card order, skipping reports', () => {
    expect(mediaStep(proof, 'a', 1)).toBe('v')
    expect(mediaStep(proof, 'v', 1)).toBe('b')
    expect(mediaStep(proof, 'b', -1)).toBe('v')
    expect(mediaStep(proof, 'v', -1)).toBe('a')
  })

  test('stops at either end and for an id that is not media', () => {
    expect(mediaStep(proof, 'a', -1)).toBe(null)
    expect(mediaStep(proof, 'b', 1)).toBe(null)
    expect(mediaStep(proof, 'r', 1)).toBe(null)
  })
})

describe('modelView', () => {
  test('names the model the agent reported, beside what the runner pinned', () => {
    const view = (model: string | null, reported_model: string | null, state: 'running' | 'succeeded' = 'running') =>
      modelView({ model, reported_model, state })
    expect(view(null, 'claude-opus-5-5')).toEqual({ name: 'claude-opus-5-5', note: 'agent default' })
    expect(view('haiku', 'claude-haiku-5-5')).toEqual({ name: 'claude-haiku-5-5', note: 'pinned haiku' })
    expect(view('gpt-6-luna', 'gpt-6-luna')).toEqual({ name: 'gpt-6-luna', note: 'pinned' })
    expect(view(null, null)).toEqual({ name: null, note: 'waiting for the agent' })
    expect(view('haiku', null)).toEqual({ name: null, note: 'waiting for the agent, pinned haiku' })
    expect(view(null, null, 'succeeded')).toEqual({ name: null, note: 'agent default' })
    expect(view('haiku', null, 'succeeded')).toEqual({ name: 'haiku', note: 'pinned' })
  })
})
