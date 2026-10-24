// What the Pull requests page shows, as pure functions over the API's types. No React here.
import type { components } from '@/api/schema'

export type OpenPullRequest = components['schemas']['OpenPullRequest']

export type SectionKey = 'waits-on-merge' | 'in-review' | 'draft' | 'names-no-issue'

export interface Section {
  key: SectionKey
  title: string
  note: string | null
  items: OpenPullRequest[]
}

// In order: a pull request is in the first section that fits, so an unlinked draft is in `names-no-issue`.
const SECTIONS: readonly (Omit<Section, 'items'> & { fits: (pr: OpenPullRequest) => boolean })[] = [
  {
    key: 'waits-on-merge',
    title: 'Waits on a merge on GitHub',
    note: 'Issues in Merging. They move to Done when GitHub reports the merge.',
    fits: (pr) => pr.waits_on_merge,
  },
  { key: 'in-review', title: 'In review', note: null, fits: (pr) => !!pr.issue && !pr.draft },
  { key: 'draft', title: 'Draft', note: null, fits: (pr) => !!pr.issue && pr.draft },
  {
    key: 'names-no-issue',
    title: 'Names no issue',
    note: 'As in pl pr unlinked: no issue id in the title, body or branch.',
    fits: () => true,
  },
]

/** The non-empty sections in order, each keeping the items' order. */
export function sections(items: readonly OpenPullRequest[]): Section[] {
  const placed = new Map<SectionKey, OpenPullRequest[]>()
  for (const pr of items) {
    const section = SECTIONS.find((s) => s.fits(pr))
    if (section) placed.set(section.key, [...(placed.get(section.key) ?? []), pr])
  }
  return SECTIONS.flatMap(({ key, title, note }) => {
    const found = placed.get(key)
    return found ? [{ key, title, note, items: found }] : []
  })
}

export type Tone = 'ok' | 'bad' | 'warn' | 'muted'

// GitHub's `mergeable_state`, as recorded.
const MERGE_STATES = new Map<string, { label: string; tone: Tone }>([
  ['clean', { label: 'Ready to merge', tone: 'ok' }],
  ['has_hooks', { label: 'Ready to merge', tone: 'ok' }],
  ['unstable', { label: 'Checks failing', tone: 'bad' }],
  ['blocked', { label: 'Blocked by review', tone: 'warn' }],
  ['behind', { label: 'Behind the base branch', tone: 'warn' }],
  ['dirty', { label: 'Conflicts', tone: 'bad' }],
  ['draft', { label: 'Draft', tone: 'muted' }],
])
const UNKNOWN = { label: 'Merge state not known yet', tone: 'muted' } as const

export const mergeLabel = (state: string): string => (MERGE_STATES.get(state) ?? UNKNOWN).label
export const mergeTone = (state: string): Tone => (MERGE_STATES.get(state) ?? UNKNOWN).tone

/** The notice on a row that waits on a merge, after its bold lead `Waits on a merge on GitHub.` */
export const waitNotice = (pr: OpenPullRequest): string =>
  `${pr.issue?.id ?? 'Its issue'} moves to ${pr.issue?.on_merge_to ?? 'its next status'} when GitHub reports the merge.${pr.merge_state === 'dirty' ? ' Resolve the conflicts first.' : ''}`

export const threadsText = (n: number): string => `${String(n)} open thread${n === 1 ? '' : 's'}`
