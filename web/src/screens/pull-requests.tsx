import { useEffect, useLayoutEffect, useMemo } from 'react'
import { useNavigate } from '@tanstack/react-router'
import { GitMerge } from 'lucide-react'
import { cn } from 'cn'
import { AccountMenu } from '@/components/account-menu'
import { AppHeader, inField } from '@/components/app-header'
import { GithubLink, StatusDot } from '@/components/atoms'
import { useEventStream, useInbox, useIssues, usePullRequests } from '@/lib/queries'
import type { Workspace } from '@/model/workspaces'
import {
  mergeLabel,
  mergeTone,
  sections,
  threadsText,
  waitNotice,
  type OpenPullRequest,
  type Tone,
} from '@/model/pull-requests'

const TONE: Record<Tone, string> = {
  ok: 'text-ok bg-ok-soft',
  bad: 'text-bad bg-bad-soft',
  warn: 'text-warn bg-warn-soft border-warn-line',
  muted: 'text-ink-3 bg-secondary',
}

/** Records a `pl:render` measure for each commit of the page with its pull requests, as the list does. */
function useRenderMeasure(rows: number | undefined) {
  const start = 'pl:render-start:pull-requests'
  performance.mark(start)
  useLayoutEffect(() => {
    if (rows !== undefined) performance.measure('pl:render', { start, detail: { layout: 'pull-requests', rows } })
  })
}

function PrLink({ pr, text }: { pr: OpenPullRequest; text: string }) {
  return pr.url ? <GithubLink url={pr.url} number={pr.number} text={text} /> : null
}

function Row({ pr, onOpen }: { pr: OpenPullRequest; onOpen: (pr: OpenPullRequest) => void }) {
  const tone = mergeTone(pr.merge_state)
  const checksTone =
    pr.checks_passed === pr.checks_total ? 'text-ok' : pr.draft ? 'text-ink-3' : 'text-bad'
  return (
    <div
      tabIndex={0}
      data-pr-row={pr.number}
      aria-label={`Pull request #${String(pr.number)}: ${pr.title}, ${pr.issue ? pr.issue.id : 'names no issue'}`}
      className="grid grid-cols-[56px_minmax(0,1fr)] items-center gap-x-3 gap-y-1.5 border-t px-2.5 py-2 first:border-t-0 hover:bg-secondary focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-ring max-lg:[&>*:not(:first-child)]:col-start-2 lg:grid-cols-[64px_minmax(0,1fr)_200px_190px_160px_130px]"
      onKeyDown={(event) => {
        if (event.key !== 'Enter' || event.target !== event.currentTarget) return
        event.preventDefault()
        onOpen(pr)
      }}
    >
      <span className="font-mono text-meta whitespace-nowrap text-ink-3">#{pr.number}</span>
      <span className="grid justify-items-start gap-0.75">
        <span className="font-medium wrap-anywhere">{pr.title}</span>
        <span className="rounded-sm bg-secondary px-1.5 py-px font-mono text-caption wrap-anywhere text-muted-foreground">
          {pr.branch}
        </span>
      </span>
      {pr.issue ? (
        <span className="inline-flex flex-wrap items-center gap-x-2 gap-y-1 text-meta">
          <span className="font-mono">{pr.issue.id}</span>
          <span className="inline-flex items-center gap-1.25 whitespace-nowrap text-muted-foreground">
            <StatusDot status={pr.issue.status} />
            {pr.issue.status_name}
          </span>
        </span>
      ) : (
        <span className="text-meta text-warn italic">names no issue</span>
      )}
      <span className="flex flex-wrap gap-x-2.5 gap-y-0.5 text-meta whitespace-nowrap">
        <span className={checksTone} title="Checks passed of total">
          checks {pr.checks_passed}/{pr.checks_total}
        </span>
        <span className={pr.open_threads > 0 ? 'text-warn' : 'text-ink-3'}>{threadsText(pr.open_threads)}</span>
      </span>
      <span
        className={cn(
          'justify-self-start rounded-full border px-2 py-px text-caption font-medium whitespace-nowrap',
          TONE[tone],
        )}
        data-merge-state={pr.merge_state}
      >
        {mergeLabel(pr.merge_state)}
      </span>
      <span>
        <PrLink pr={pr} text="Open on GitHub" />
      </span>
      {pr.waits_on_merge && (
        <div
          className="col-start-2 col-end-[-1] flex flex-wrap items-center gap-x-2.5 gap-y-1.5 rounded-md border border-warn-line bg-warn-soft px-2.5 py-1.5 text-meta"
          data-waits-on-merge
        >
          <GitMerge className="size-3.5 flex-none text-warn" aria-hidden="true" />
          <span className="flex-[1_1_200px]">
            <b className="text-warn">Waits on a merge on GitHub.</b> {waitNotice(pr)}
          </span>
          <PrLink pr={pr} text="Merge on GitHub" />
        </div>
      )}
    </div>
  )
}

export function PullRequestsScreen({ workspace, me }: { workspace: Workspace; me: string }) {
  const issues = useIssues(workspace.id)
  useEventStream(workspace.id, issues.isSuccess)
  const pullRequests = usePullRequests(workspace.id, issues.isSuccess)
  const inbox = useInbox(workspace.id, issues.isSuccess)
  const navigate = useNavigate()
  const grouped = useMemo(() => sections(pullRequests.data ?? []), [pullRequests.data])
  useRenderMeasure(pullRequests.data?.length)

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== 'Escape' || inField(event.target)) return
      if (event.defaultPrevented || document.querySelector('[role="dialog"], [role="menu"]')) return
      event.preventDefault()
      void navigate({ to: '/w/$prefix', params: { prefix: workspace.issue_prefix } })
    }
    document.addEventListener('keydown', onKey)
    return () => {
      document.removeEventListener('keydown', onKey)
    }
  }, [navigate, workspace.issue_prefix])

  const open = (pr: OpenPullRequest) => {
    if (pr.issue) void navigate({ to: '/issues/$id', params: { id: pr.issue.id } })
    else if (pr.url) window.open(pr.url, '_blank', 'noopener')
  }
  const failed = issues.error ?? pullRequests.error

  return (
    <div className="flex min-h-screen flex-col">
      <AppHeader workspace={workspace} current="pull-requests" pullRequests={pullRequests.data?.length} inbox={inbox.data?.unread}>
        <AccountMenu me={me} className="ml-auto" />
      </AppHeader>

      <main className="mx-auto grid w-full max-w-320 flex-1 content-start gap-3 px-3 py-2 sm:px-4 sm:py-3" aria-live="polite">
        <h1 className="flex flex-wrap items-baseline gap-x-2.5 gap-y-1 text-title font-semibold">
          Pull requests
          {pullRequests.data && (
            <span className="text-meta font-normal text-ink-3">{pullRequests.data.length} open</span>
          )}
        </h1>
        {failed ? (
          <p className="p-6 text-center text-bad" role="alert">
            Could not load the pull requests: {String(failed)}
          </p>
        ) : !pullRequests.data ? (
          <p className="p-6 text-center text-ink-3">Loading pull requests…</p>
        ) : grouped.length === 0 ? (
          <p className="rounded-lg border border-dashed bg-card px-5 py-7 text-center text-muted-foreground" data-empty>
            No open pull requests. They appear here as GitHub reports them.
          </p>
        ) : (
          grouped.map((section) => (
            <section key={section.key} data-section={section.key} aria-labelledby={`section-${section.key}`}>
              <h2 id={`section-${section.key}`} className="mt-1 mb-1.5 flex items-center gap-2 font-semibold">
                {section.title} <span className="font-medium text-ink-3">{section.items.length}</span>
              </h2>
              {section.note && <p className="mb-2 text-meta text-ink-3">{section.note}</p>}
              <div className="overflow-hidden rounded-lg border bg-card">
                {section.items.map((pr) => (
                  <Row key={pr.number} pr={pr} onOpen={open} />
                ))}
              </div>
            </section>
          ))
        )}
      </main>

      <footer className="sticky bottom-0 z-5 flex flex-wrap items-center gap-x-3 gap-y-1 border-t bg-card px-3 py-1.5 text-caption text-muted-foreground sm:gap-x-4 sm:px-4 sm:py-2 sm:text-meta">
        <span className="font-semibold text-foreground">Pull requests shortcuts</span>
        {(
          [
            ['Tab', 'next pull request'],
            ['Enter', 'open issue or pull request'],
            ['g p', 'pull requests'],
            ['Esc', 'issues'],
          ] as const
        ).map(([keys, what]) => (
          <span key={what} className="inline-flex items-center gap-1.25 whitespace-nowrap">
            {keys.split(' ').map((key) => (
              <kbd key={key}>{key}</kbd>
            ))}{' '}
            {what}
          </span>
        ))}
      </footer>
    </div>
  )
}
