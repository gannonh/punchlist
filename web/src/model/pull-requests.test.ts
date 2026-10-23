import { describe, expect, it } from 'vitest'
import { mergeLabel, sections, waitNotice, type OpenPullRequest } from './pull-requests'

// The prototype's rows (`prototypes/pull-requests/data.js`), as the server answers them.
const pr = (
  number: number,
  draft: boolean,
  issue: [string, string, string] | null,
  merge_state: string,
): OpenPullRequest => ({
  number,
  title: `Pull request ${String(number)}`,
  branch: `feature/${String(number)}`,
  url: `https://github.com/gannonh/punchlist-sandbox/pull/${String(number)}`,
  draft,
  merge_state,
  checks_passed: 6,
  checks_total: 6,
  open_threads: 0,
  updated_at: '2026-10-09T18:00:00Z',
  issue: issue && { id: issue[0], title: 'An issue', status: issue[1], status_name: issue[2] },
  waits_on_merge: !draft && issue?.[1] === 'merging',
})

const ITEMS = [
  pr(241, false, ['PL-1', 'merging', 'Merging'], 'clean'),
  pr(238, false, ['PL-3', 'merging', 'Merging'], 'dirty'),
  pr(244, false, ['PL-4', 'human_review', 'Human Review'], 'blocked'),
  pr(246, false, ['PL-7', 'agent_review', 'Agent Review'], 'unstable'),
  pr(250, false, ['PL-12', 'agent_review', 'Agent Review'], 'behind'),
  pr(247, true, ['PL-9', 'in_progress', 'In Progress'], 'draft'),
  pr(249, false, null, 'clean'),
]

describe('sections', () => {
  it('puts each pull request in the first section that fits, in order, omitting empty ones', () => {
    expect(sections(ITEMS).map((s) => [s.key, s.items.map((p) => p.number)])).toEqual([
      ['waits-on-merge', [241, 238]],
      ['in-review', [244, 246, 250]],
      ['draft', [247]],
      ['names-no-issue', [249]],
    ])
    expect(sections([pr(1, true, null, 'draft')]).map((s) => [s.key, s.title])).toEqual([
      ['names-no-issue', 'Names no issue'],
    ])
    expect(sections([])).toEqual([])
  })
})

describe('mergeLabel', () => {
  it("reads GitHub's mergeable_state, and anything else as not known yet", () => {
    expect(mergeLabel('has_hooks')).toBe('Ready to merge')
    expect(mergeLabel('behind')).toBe('Behind the base branch')
    expect(mergeLabel('unknown')).toBe('Merge state not known yet')
    expect(mergeLabel('constructor')).toBe('Merge state not known yet')
  })
})

describe('waitNotice', () => {
  it('names the issue, and asks for the conflicts first when the merge state is dirty', () => {
    expect(waitNotice(ITEMS[0] as OpenPullRequest)).toBe('PL-1 moves to Done when GitHub reports the merge.')
    expect(waitNotice(ITEMS[1] as OpenPullRequest)).toBe(
      'PL-3 moves to Done when GitHub reports the merge. Resolve the conflicts first.',
    )
  })
})
