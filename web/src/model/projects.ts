// What the projects screen shows, as pure functions over the API's types. No React here.
import type { components } from '@/api/schema'
import type { IssueRow, StatusInfo } from '@/model/issues'

export type Project = components['schemas']['Project']
export type Milestone = components['schemas']['Milestone']

export interface Progress {
  /** The milestone's issues in `done`. */
  done: number
  /** The milestone's issues, except those in `canceled`. */
  total: number
  /** The milestone's issues in `canceled`, left out of the total. */
  canceled: number
  /** Each workflow status with any of the counted issues, in the workflow's order: the bar's segments. */
  byStatus: { key: string; name: string; count: number }[]
  /** The counted issues, in the workflow's status order. */
  issues: IssueRow[]
}

// ponytail: reads the status keys `done` and `canceled`, so a workflow without them shows no progress. The upgrade
// is a flag in `StatusInfo` that tells a done status from a canceled one (`closed` covers both).
const DONE = 'done'
const CANCELED = 'canceled'

/** How far a milestone has come: its issues in `done` out of all but those in `canceled`. */
export function milestoneProgress(
  rows: readonly IssueRow[],
  statuses: readonly StatusInfo[],
  milestoneId: string,
): Progress {
  const mine = rows.filter((row) => row.issue.milestone?.id === milestoneId)
  const issues = mine.filter((row) => row.issue.status !== CANCELED)
  const order = new Map(statuses.map((status, index) => [status.key, index]))
  const at = (row: IssueRow) => order.get(row.issue.status) ?? statuses.length
  return {
    done: issues.filter((row) => row.issue.status === DONE).length,
    total: issues.length,
    canceled: mine.length - issues.length,
    byStatus: statuses
      .map(({ key, name }) => ({ key, name, count: issues.filter((row) => row.issue.status === key).length }))
      .filter((status) => status.count > 0),
    issues: issues.toSorted((a, b) => at(a) - at(b)),
  }
}
