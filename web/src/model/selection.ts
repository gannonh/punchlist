// The list's selection and what a change to every selected issue does, as pure functions. No React here.
import type { ErrorBody, IssueRow, StatusInfo } from './issues'
import type { TransitionList } from './issue-page'

/** Issue ids. */
export type Selection = ReadonlySet<string>

export function toggle(selection: Selection, id: string): Selection {
  const next = new Set(selection)
  if (!next.delete(id)) next.add(id)
  return next
}

/** Adds every id from `from` to `to` in `order`, both included, whichever comes first. */
export function addRange(selection: Selection, order: readonly string[], from: string, to: string): Selection {
  const a = order.indexOf(from)
  const b = order.indexOf(to)
  if (a < 0 || b < 0) return selection
  return new Set([...selection, ...order.slice(Math.min(a, b), Math.max(a, b) + 1)])
}

/** The ids still shown. The same selection when none is hidden, so a render can tell nothing dropped. */
export function keepShown(selection: Selection, shown: readonly string[]): Selection {
  const kept = shown.filter((id) => selection.has(id))
  return kept.length === selection.size ? selection : new Set(kept)
}

/** A choice in a picker: true when every target has it, `'indeterminate'` when some do. */
export type Mark = boolean | 'indeterminate'

export function mark(has: readonly boolean[]): Mark {
  const count = has.filter(Boolean).length
  return count === 0 ? false : count === has.length ? true : 'indeterminate'
}

export const labelMark = (labelSets: readonly (readonly string[])[], label: string): Mark =>
  mark(labelSets.map((labels) => labels.includes(label)))

/** `labels` with `label` added at the end or removed, the others as they were, each once. */
export function withLabel(labels: readonly string[], label: string, add: boolean): string[] {
  const rest = labels.filter((l) => l !== label)
  return add ? [...rest, label] : rest
}

/** Why one issue of a bulk edit was refused, as the toast names it: `PL-12 is in Human Review, locked`. */
export const refusedText = (id: string, statusName: string, error: ErrorBody): string =>
  error.code === 'locked' ? `${id} is in ${statusName}, locked` : `${id}: ${error.message}`

/** The one toast of a bulk edit: `Set Project to Punchlist on 5 issues`, or `4 updated; PL-12 is in …`. */
export function bulkSummary(action: string, updated: number, refused: readonly string[]): string {
  if (refused.length > 0) return [`${String(updated)} updated`, ...refused].join('; ')
  return `${action} on ${String(updated)} ${updated === 1 ? 'issue' : 'issues'}`
}

/** One issue's answer to a move of several: `error` is its refusal, absent when it moved. */
export interface MoveResult {
  id: string
  error?: ErrorBody
}

const refusalName = (error: ErrorBody) =>
  error.code === 'gate_failed' && error.gate ? `gate ${error.gate} failed` : error.message

/** The one toast of a move of several issues: `Moved 2 to Todo; PL-12 refused: gate ci_green failed`. */
export function moveSummary(toName: string, results: readonly MoveResult[]): string {
  const refused = results.flatMap(({ id, error }) => (error ? [`${id} refused: ${refusalName(error)}`] : []))
  return [`Moved ${String(results.length - refused.length)} to ${toName}`, ...refused].join('; ')
}

/**
 * Why the caller can move none of `issues` to `to`, from each issue's `GET /api/issues/{id}/transitions` (`lists`,
 * in the same order, `undefined` while it loads): each distinct reason, or null when one issue can move, or may
 * once its moves load. A move a gate blocks can still be requested, since the request reads GitHub first.
 */
export function moveDisabled(
  to: StatusInfo,
  issues: readonly IssueRow['issue'][],
  lists: readonly (TransitionList | undefined)[],
): string | null {
  const reasons = new Set<string>()
  for (const [index, issue] of issues.entries()) {
    if (issue.status === to.key) {
      reasons.add(`already in ${to.name}`)
      continue
    }
    const list = lists[index]
    if (!list) return null
    const verdict = list.transitions.find((option) => option.to.key === to.key)?.verdict
    if (!verdict) reasons.add(`${issue.status_name} → ${to.name} cannot be requested`)
    else if (verdict.kind === 'refused') reasons.add(verdict.message)
    else return null
  }
  return reasons.size > 0 ? [...reasons].join('; ') : null
}
