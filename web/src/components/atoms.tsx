import type { ComponentProps, KeyboardEvent, SyntheticEvent } from 'react'
import {
  Bot,
  GitMerge,
  GitPullRequest,
  GitPullRequestClosed,
  GitPullRequestDraft,
  Loader2,
  Lock,
  X,
} from 'lucide-react'
import { cn } from 'cn'
import type {
  AssigneeView,
  GateView,
  PullRequestView,
  RunView,
  StatusInfo,
  Transition,
  WaitsOnMerge,
} from '@/model/issues'
import { peopleOnly } from '@/model/issues'

export function StatusDot({ status }: { status: string }) {
  return <span className="status-dot" data-status={status} aria-hidden="true" />
}

export function LockBadge({ status }: { status: StatusInfo }) {
  if (!peopleOnly(status)) return null
  return (
    <span
      className="inline-flex items-center gap-1 rounded-full border border-warn-line bg-warn-soft px-1.75 py-px text-caption font-medium whitespace-nowrap text-warn"
      title={`Locked to people: ${status.locked_to.join(' and ')} cannot act here`}
    >
      <Lock className="size-2.5" aria-hidden="true" /> people only
    </span>
  )
}

export function Assignee({ view, short = false }: { view: AssigneeView; short?: boolean }) {
  switch (view.kind) {
    case 'unassigned':
      return (
        <span className="assignee inline-flex items-center gap-1.5 whitespace-nowrap" data-assignee="unassigned">
          <span className="size-5 flex-none rounded-full border border-dashed border-zinc-400" aria-hidden="true" />
          <span className="text-ink-3 italic">Unassigned</span>
        </span>
      )
    case 'agent':
      return (
        <span
          className="assignee inline-flex items-center gap-1.5 whitespace-nowrap"
          data-assignee="agent"
          title={`Agent: ${view.name}`}
        >
          <span
            className="grid size-5 flex-none place-items-center rounded-[4px] border border-agent-line bg-agent-soft text-agent"
            aria-hidden="true"
          >
            <Bot className="size-3" />
          </span>
          <span className="rounded-[4px] border border-agent-line bg-agent-soft px-1.25 text-caption font-medium text-agent">
            {view.label}
          </span>
          <span className="sr-only">agent</span>
        </span>
      )
    case 'person':
      return (
        <span
          className="assignee inline-flex min-w-0 items-center gap-1.5 whitespace-nowrap"
          data-assignee="person"
          title={`Person: ${view.label}`}
        >
          <span
            className="grid size-5 flex-none place-items-center rounded-full bg-zinc-200 text-[9px] leading-none font-semibold text-zinc-800"
            aria-hidden="true"
          >
            {view.initials}
          </span>
          <span className="truncate text-muted-foreground">{short ? view.label.split(' ')[0] : view.label}</span>
          <span className="sr-only">person</span>
        </span>
      )
  }
}

const RUN_COLOR: Partial<Record<RunView['state'], string>> = { running: 'text-ok', failed: 'text-bad', waiting: 'text-warn' }

export function Run({ view, detail = false }: { view: RunView; detail?: boolean }) {
  return (
    <span
      className={cn(
        `run-${view.state} inline-flex items-center gap-1.5 truncate text-meta whitespace-nowrap`,
        RUN_COLOR[view.state] ?? 'text-muted-foreground',
      )}
      data-run={view.state}
      title={view.detail ?? view.label}
    >
      <i className="run-dot" aria-hidden="true" />
      {view.label}
      {detail && view.detail && <span className="text-ink-3">{view.detail}</span>}
    </span>
  )
}

const PR_ICON = {
  draft: GitPullRequestDraft,
  open: GitPullRequest,
  merged: GitMerge,
  closed: GitPullRequestClosed,
} as const
const PR_COLOR = { draft: 'text-zinc-500', open: 'text-ok', merged: 'text-agent', closed: 'text-bad' } as const
const CHECK_COLOR = { passing: 'bg-ok', failing: 'bg-bad', pending: 'bg-warn' } as const

/**
 * A link out to a pull request on GitHub, in a new tab. A click or `Enter` on it stays on the link: it does not pick
 * the row it is in or reach the page's keys.
 */
export function GithubLink({
  url,
  number,
  text,
  children,
  className = 'text-meta font-medium text-primary',
  ...props
}: ComponentProps<'a'> & { url: string; number: number; text: string }) {
  return (
    <a
      href={url}
      target="_blank"
      rel="noopener noreferrer"
      aria-label={`${text}: pull request #${String(number)}`}
      className={`external-link inline-flex items-center gap-1 whitespace-nowrap hover:underline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring ${className}`}
      onClick={stopPropagation}
      onKeyDown={stopEnter}
      {...props}
    >
      {children ?? text}
    </a>
  )
}

const stopPropagation = (event: SyntheticEvent) => {
  event.stopPropagation()
}
const stopEnter = (event: KeyboardEvent) => {
  if (event.key === 'Enter') event.stopPropagation()
}

/** `link` makes the chip a link to the pull request on GitHub when it has a plain GitHub URL. */
export function PullRequest({ view, link = false }: { view: PullRequestView; link?: boolean }) {
  if (view.state === 'none')
    return (
      <span className="truncate text-meta whitespace-nowrap text-ink-3" data-pr="none">
        {view.label}
      </span>
    )
  const Icon = PR_ICON[view.state]
  const chip = (
    <>
      <Icon className={cn('size-3.25 flex-none', PR_COLOR[view.state])} aria-hidden="true" />
      {view.label}
    </>
  )
  return (
    <span
      className="inline-flex items-center gap-1.25 truncate text-meta whitespace-nowrap text-muted-foreground"
      data-pr={view.state}
      title={view.title}
    >
      {link && view.url && view.number !== null ? (
        <GithubLink url={view.url} number={view.number} text="Open on GitHub" className="text-muted-foreground" data-pr-link>
          {chip}
        </GithubLink>
      ) : (
        chip
      )}
      {view.checks !== 'none' && (
        <i
          className={cn('inline-block size-1.75 rounded-full', CHECK_COLOR[view.checks])}
          aria-label={`checks ${view.checks}`}
        />
      )}
    </span>
  )
}

/** In place of a row's gate summary: nothing gates the move, the merge on GitHub makes it. */
export function MergeWait({ id, number, wait }: { id: string; number: number | null; wait: WaitsOnMerge }) {
  return (
    <span
      className="inline-flex flex-wrap items-center gap-x-1.5 gap-y-0.5 text-meta font-medium whitespace-nowrap text-warn"
      title={`${id} moves to ${wait.to} when GitHub reports the merge`}
    >
      Waits on the merge
      {wait.url && number !== null && <GithubLink url={wait.url} number={number} text="Merge on GitHub" data-merge-link />}
    </span>
  )
}

export function Gates({ view }: { view: GateView }) {
  const all = view.total > 0 && view.passed === view.total
  return (
    <span
      className={cn(
        'inline-flex items-center gap-1.5 truncate text-meta whitespace-nowrap',
        view.total === 0 ? 'text-ink-3' : all ? 'text-ok' : 'text-muted-foreground',
      )}
      data-gates={`${String(view.passed)}/${String(view.total)}`}
      title={view.title}
    >
      {view.total > 0 && (
        <span className="inline-flex gap-0.5" aria-hidden="true">
          {Array.from({ length: view.total }, (_, i) => (
            <b key={i} className={cn('inline-block h-1.5 w-2.5 rounded-[2px]', i < view.passed ? 'bg-ok' : 'bg-bad')} />
          ))}
        </span>
      )}
      {view.text}
    </span>
  )
}

export function Labels({ labels }: { labels: readonly string[] }) {
  return labels.map((label) => (
    <span
      key={label}
      className="ml-1.5 rounded-full border px-1.5 text-caption font-normal whitespace-nowrap text-ink-3"
    >
      {label}
    </span>
  ))
}

export function TransitionNote({ transition, onDismiss }: { transition: Transition; onDismiss: () => void }) {
  if (transition.state === 'pending')
    return (
      <span
        className="transition-note inline-flex items-center gap-1.5 text-meta text-primary"
        data-transition="pending"
        role="status"
      >
        <Loader2 className="size-3 animate-spin" aria-hidden="true" />
        Moving to {transition.toName}…
      </span>
    )
  return (
    <span
      className="transition-note flex items-start gap-1.5 rounded-md border border-bad/30 bg-bad-soft px-2 py-1 text-meta text-bad"
      data-transition="refused"
      role="alert"
    >
      <span className="min-w-0 flex-1 break-words">
        {transition.toName}: {transition.text}
      </span>
      <button
        type="button"
        className="flex-none cursor-pointer rounded-sm opacity-70 hover:opacity-100"
        aria-label="Dismiss"
        onClick={(event) => {
          event.stopPropagation()
          onDismiss()
        }}
      >
        <X className="size-3.5" />
      </button>
    </span>
  )
}
