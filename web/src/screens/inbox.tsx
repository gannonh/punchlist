import { useEffect, useLayoutEffect, useMemo } from 'react'
import { Link, useNavigate } from '@tanstack/react-router'
import { cn } from 'cn'
import { AccountMenu } from '@/components/account-menu'
import { AppHeader, inField } from '@/components/app-header'
import { StatusDot } from '@/components/atoms'
import { useEventStream, useInbox, useIssues, useMarkRead, usePullRequests } from '@/lib/queries'
import { ageText, canRunAgain, ordered, refusalParts, runLines, type InboxItem } from '@/model/inbox'
import { agentName } from '@/model/issues'
import type { Workspace } from '@/model/workspaces'
import { RunAgain } from '@/screens/issue'
import { useNow } from '@/screens/issues'

/** Records a `pl:render` measure for each commit of the page with its items, as the list does. */
function useRenderMeasure(items: number | undefined) {
  const start = 'pl:render-start:inbox'
  performance.mark(start)
  useLayoutEffect(() => {
    if (items !== undefined) performance.measure('pl:render', { start, detail: { layout: 'inbox', items } })
  })
}

const KIND = { waiting: 'Waiting on you', run: 'Run needs you', refusal: 'Refused transition' } as const

const EMPTY = {
  waiting: 'Nothing waits on you.',
  run: 'No run needs you.',
  refusal: 'No agent of yours was refused.',
} as const

const KIND_STYLE = {
  waiting: 'border-warn-line bg-warn-soft text-warn',
  run: 'border-border bg-bad-soft text-bad',
  refusal: 'border-agent-line bg-agent-soft text-agent',
} as const

function Item({ item, now, onOpen }: { item: InboxItem; now: Date; onOpen: (item: InboxItem) => void }) {
  const { issue } = item
  // A button cannot sit in a link, so Run again is the link's sibling in the item's row.
  return (
    <div className="flex items-center border-t first:border-t-0 hover:bg-secondary">
      <Link
        to="/issues/$id"
        params={{ id: issue.id }}
        data-item={issue.id}
        data-kind={item.kind}
        data-unread={item.unread || undefined}
        aria-label={`${KIND[item.kind]}: ${issue.id} ${issue.title}, ${issue.status_name}${item.unread ? ', unread' : ''}`}
        className="grid min-w-0 flex-1 gap-1.5 px-3 py-2.5 focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-ring"
        onClick={(event) => {
          if (event.metaKey || event.ctrlKey || event.shiftKey || event.button !== 0) return
          event.preventDefault()
          onOpen(item)
        }}
      >
        <span className="flex flex-wrap items-center gap-x-2.5 gap-y-1 text-meta">
          <span
            className={cn('inline-block size-2 flex-none rounded-full', item.unread ? 'bg-primary' : 'bg-transparent')}
            aria-hidden="true"
          />
          <span className="font-mono">{issue.id}</span>
          <span
            className={cn(
              'rounded-full border px-2 py-px text-caption font-medium whitespace-nowrap',
              KIND_STYLE[item.kind],
            )}
          >
            {KIND[item.kind]}
          </span>
          <span className="inline-flex items-center gap-1.25 whitespace-nowrap text-muted-foreground" data-field="status">
            <StatusDot status={issue.status} />
            {issue.status_name}
          </span>
          <span className="ml-auto text-ink-3" data-age title={item.since}>
            {ageText(item, now)}
          </span>
        </span>
        <span className={cn('wrap-anywhere', item.unread ? 'font-semibold' : 'font-medium')}>{issue.title}</span>
        {item.kind === 'run' && (
          <span className="grid gap-0.5 text-meta wrap-anywhere text-muted-foreground" data-cause>
            <span>
              <b className="font-semibold text-bad" data-agent>
                {agentName(item.agent)}
              </b>{' '}
              run on your runner
            </span>
            {runLines(item).map((line) => (
              <span key={line} data-run-line>
                {line}
              </span>
            ))}
          </span>
        )}
        {item.kind === 'refusal' && <RefusalCause parts={refusalParts(item)} />}
      </Link>
      {canRunAgain(item) && (
        <span className="flex-none pr-3">
          <RunAgain id={issue.id} />
        </span>
      )}
    </div>
  )
}

function RefusalCause({ parts }: { parts: ReturnType<typeof refusalParts> }) {
  return (
    <span className="border-l-3 border-agent-line pl-2 text-meta wrap-anywhere text-muted-foreground" data-cause>
      <span className="font-semibold text-foreground" data-agent>
        {parts.agent}
      </span>{' '}
      asked for <span data-move>{parts.move}</span>. Refused: gate <code data-gate>{parts.gate}</code> failed:{' '}
      <span data-refusal-reason>{parts.reason}</span>
    </span>
  )
}

export function InboxScreen({ workspace, me }: { workspace: Workspace; me: string }) {
  const issues = useIssues(workspace.id)
  useEventStream(workspace.id, issues.isSuccess)
  const pullRequests = usePullRequests(workspace.id, issues.isSuccess)
  const inbox = useInbox(workspace.id, true)
  const markRead = useMarkRead(workspace.id)
  const navigate = useNavigate()
  const now = useNow()
  const items = useMemo(() => ordered(inbox.data?.items ?? []), [inbox.data])
  useRenderMeasure(inbox.data?.items.length)

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== 'Escape' || inField(event.target)) return
      if (document.querySelector('[role="dialog"], [role="menu"]')) return
      event.preventDefault()
      void navigate({ to: '/w/$prefix', params: { prefix: workspace.issue_prefix } })
    }
    document.addEventListener('keydown', onKey)
    return () => {
      document.removeEventListener('keydown', onKey)
    }
  }, [navigate, workspace.issue_prefix])

  // Marked read first, so the list's count is lower on the way back; the issue opens either way.
  const open = (item: InboxItem) => {
    void markRead
      .mutateAsync(item)
      .catch(() => undefined)
      .then(() => navigate({ to: '/issues/$id', params: { id: item.issue.id } }))
  }

  return (
    <div className="flex min-h-screen flex-col">
      <AppHeader
        workspace={workspace}
        current="inbox"
        pullRequests={pullRequests.data?.length}
        inbox={inbox.data?.unread}
      >
        <AccountMenu me={me} className="ml-auto" />
      </AppHeader>

      <main className="mx-auto grid w-full max-w-320 flex-1 content-start gap-3 px-3 py-2 sm:px-4 sm:py-3" aria-live="polite">
        <h1 className="flex flex-wrap items-baseline gap-x-2.5 gap-y-1 text-title font-semibold">
          Inbox
          {inbox.data && (
            <span className="text-meta font-normal text-ink-3">
              {inbox.data.items.length} items · {inbox.data.unread} unread
            </span>
          )}
        </h1>
        <p className="text-meta text-muted-foreground">
          Issues in a status only a person moves on, issues your agents' runs left with nothing to run again, and moves
          your agents asked for that a gate refused. An item leaves when its issue moves on; opening one marks it read.
        </p>
        {inbox.error ? (
          <p className="p-6 text-center text-bad" role="alert">
            Could not load the inbox: {String(inbox.error)}
          </p>
        ) : !inbox.data ? (
          <p className="p-6 text-center text-ink-3">Loading the inbox…</p>
        ) : (
          (['refusal', 'run', 'waiting'] as const).map((kind) => {
            const its = items.filter((item) => item.kind === kind)
            return (
              <section key={kind} data-section={kind} aria-labelledby={`section-${kind}`}>
                <h2 id={`section-${kind}`} className="mt-1 mb-1.5 flex items-center gap-2 font-semibold">
                  {KIND[kind]} <span className="font-medium text-ink-3">{its.length}</span>
                </h2>
                {its.length === 0 ? (
                  <p className="rounded-lg border border-dashed bg-card px-5 py-7 text-center text-muted-foreground" data-empty>
                    {EMPTY[kind]}
                  </p>
                ) : (
                  <div className="overflow-hidden rounded-lg border bg-card">
                    {its.map((item) => (
                      <Item key={item.issue.id} item={item} now={now} onOpen={open} />
                    ))}
                  </div>
                )}
              </section>
            )
          })
        )}
      </main>

      <footer className="sticky bottom-0 z-5 flex flex-wrap items-center gap-x-3 gap-y-1 border-t bg-card px-3 py-1.5 text-caption text-muted-foreground sm:gap-x-4 sm:px-4 sm:py-2 sm:text-meta">
        <span className="font-semibold text-foreground">Inbox shortcuts</span>
        {(
          [
            ['Tab', 'next item'],
            ['Enter', 'open issue'],
            ['i', 'inbox'],
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
