// The issue page (the approved Sidebar prototype): spec, pull request, proof and activity in the main column;
// status and moves, gates, runs and properties in a sticky rail that stacks above the main column on phones.
import { Fragment, memo, useEffect, useLayoutEffect, useMemo, useRef, useState, type ReactNode, type RefObject } from 'react'
import { useQuery } from '@tanstack/react-query'
import { Link, useLocation, useNavigate } from '@tanstack/react-router'
import { ArrowLeft, Bot, Clock, GitMerge, GitPullRequest, Lock, Paperclip, Server, HardDrive } from 'lucide-react'
import { cn } from 'cn'
import { toast } from 'sonner'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogTitle } from '@/components/ui/dialog'
import { Assignee, GithubLink, Labels, LockBadge, PullRequest, StatusDot } from '@/components/atoms'
import { WorkspaceMenu } from '@/components/workspace-menu'
import {
  issuePageQueries,
  issuesQuery,
  runLogQuery,
  useAttachProof,
  useComment,
  useEventStream,
  useMove,
  useReadFromGithub,
} from '@/lib/queries'
import { age, agentName, assigneeView, pullRequestView, waitsOnMerge, type Actor, type WaitsOnMerge } from '@/model/issues'
import type { Workspace } from '@/model/workspaces'
import {
  GATE_EVIDENCE,
  actorMark,
  dependencies,
  duration,
  eventText,
  feedEntries,
  fileSize,
  gateSummary,
  gateText,
  logView,
  mediaStep,
  modelView,
  moveView,
  mergeNotice,
  plainPullRequestUrl,
  PROOF_TYPES,
  specBlocks,
  threadView,
  tokens,
  cost,
  type ActorMark,
  type AttemptInfo,
  type CheckRun,
  type Feed,
  type GateChecklist,
  type Issue,
  type IssueEvent,
  type LogLine,
  type Proof,
  type PullRequest as PullRequestInfo,
  type ReviewThread,
  type Run,
  type TransitionList,
} from '@/model/issue-page'
import { useNow } from '@/screens/issues'

const ROLE_WORD = { person: 'person', agent: 'agent', runner: 'runner', github: 'GitHub' } as const

const inField = (target: EventTarget | null) =>
  target instanceof HTMLElement &&
  (target.isContentEditable || ['INPUT', 'SELECT', 'TEXTAREA'].includes(target.tagName))

const jump = (id: string) => document.getElementById(id)?.scrollIntoView({ block: 'start' })

function Panel({
  id,
  title,
  right,
  className,
  children,
}: {
  id?: string
  title: ReactNode
  right?: ReactNode
  className?: string
  children: ReactNode
}) {
  return (
    <section className={cn('panel', className)} id={id} data-section={id}>
      <h2 className="panel-title">
        {title}
        {right && <span className="ml-auto font-medium tracking-normal normal-case">{right}</span>}
      </h2>
      {children}
    </section>
  )
}

function StatusPill({ status, name }: { status: string; name: string }) {
  return (
    <span className="inline-flex items-center gap-1.5 rounded-full border bg-card py-0.5 pr-2.25 pl-1.75 font-semibold text-foreground">
      <StatusDot status={status} />
      {name}
    </span>
  )
}

const AVATAR: Record<ActorMark['mark'], string> = {
  person: 'rounded-full bg-zinc-200 text-zinc-800',
  agent: 'rounded-[4px] border border-agent-line bg-agent-soft text-agent',
  runner: 'rounded-[4px] border border-cyan-200 bg-cyan-50 text-cyan-700',
  github: 'rounded-full bg-zinc-800 text-white',
  server: 'rounded-[4px] border border-primary-line bg-primary-soft text-primary',
}
const CHIP: Record<ActorMark['mark'], string> = {
  person: '',
  agent: 'border-agent-line bg-agent-soft text-agent',
  runner: 'border-cyan-200 bg-cyan-50 text-cyan-700',
  github: 'border-zinc-300 bg-zinc-100 text-zinc-800',
  server: 'border-primary-line bg-primary-soft text-primary',
}

function Avatar({ mark }: { mark: ActorMark }) {
  const icon = { agent: Bot, runner: HardDrive, github: GitPullRequest, server: Server, person: null }[mark.mark]
  const Icon = icon
  return (
    <span
      className={cn(
        'relative z-1 grid size-5 flex-none place-items-center text-[8px] leading-none font-semibold shadow-[0_0_0_3px_var(--card)]',
        AVATAR[mark.mark],
      )}
      aria-hidden="true"
    >
      {Icon ? <Icon className="size-3" /> : mark.initials}
    </span>
  )
}

function Chip({ mark }: { mark: ActorMark }) {
  if (!mark.chip) return null
  return (
    <span className={cn('rounded-[4px] border px-1 text-[10px] leading-[15px] font-medium', CHIP[mark.mark])}>
      {mark.chip}
    </span>
  )
}

/** Records a `pl:render` measure for each commit of the page, as the list and the board do. */
function useRenderMeasure(events: number, attempts: number) {
  const start = 'pl:render-start:issue'
  performance.mark(start)
  useLayoutEffect(() => {
    performance.measure('pl:render', { start, detail: { layout: 'issue', events, attempts } })
  })
}

/** The link back to the list: the issue's workspace's, or `/` when the viewer has no workspace menu. */
function ListLink({ workspace, className, children }: { workspace?: Workspace; className: string; children: ReactNode }) {
  return workspace ? (
    <Link to="/w/$prefix" params={{ prefix: workspace.issue_prefix }} className={className}>
      {children}
    </Link>
  ) : (
    <Link to="/" className={className}>
      {children}
    </Link>
  )
}

/**
 * `workspace` is the issue's workspace among the person's, with its menu; an agent viewer has none. `workspaceId` is
 * the workspace whose list and stream the page reads.
 */
export function IssueScreen({
  id,
  viewer,
  workspace,
  workspaceId,
}: {
  id: string
  viewer: Actor
  workspace: Workspace | undefined
  workspaceId: string
}) {
  const person = viewer.role === 'person'
  const q = useMemo(() => issuePageQueries(id), [id])
  const issue = useQuery(q.issue)
  const events = useQuery(q.events)
  const runs = useQuery(q.runs)
  const proof = useQuery(q.proof)
  const gates = useQuery(q.gates)
  const transitions = useQuery(q.transitions)
  const pullRequests = useQuery(q.pullRequests)
  const reviewThreads = useQuery(q.reviewThreads)
  // The event stream and the list it starts after are the workspace's, which an agent's token cannot read: an
  // agent viewer reads the page again after its own writes only.
  const list = useQuery({ ...issuesQuery(workspaceId), enabled: person })
  useEventStream(workspaceId, person && list.isSuccess)
  const navigate = useNavigate()
  const now = useNow()
  const [feed, setFeed] = useState<Feed>('all')
  const composer = useRef<HTMLTextAreaElement>(null)

  const attempts = (runs.data ?? []).reduce((n, run) => n + (run.attempts ?? []).length, 0)
  useRenderMeasure(events.data?.length ?? 0, attempts)

  // `#seq-<n>` names a timeline entry, such as the comment a search matched: once the timeline is there, show it.
  const target = useLocation({ select: (location) => location.hash })
  const timelineLoaded = events.isSuccess
  useEffect(() => {
    if (target && timelineLoaded) document.getElementById(target)?.scrollIntoView({ block: 'center' })
  }, [target, timelineLoaded])

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      // An open dialog handles its own keys, `Esc` included, wherever focus is.
      if (event.defaultPrevented || event.metaKey || event.ctrlKey || event.altKey) return
      if (inField(event.target)) {
        if (event.key === 'Escape') (event.target as HTMLElement).blur()
        return
      }
      const keys: Record<string, (() => void) | undefined> = {
        g: () => {
          jump('gates')
        },
        t: () => {
          jump('timeline')
        },
        c: () => {
          jump('timeline')
          if (composer.current && !composer.current.disabled) composer.current.focus()
        },
        Escape: () =>
          void (workspace
            ? navigate({ to: '/w/$prefix', params: { prefix: workspace.issue_prefix } })
            : navigate({ to: '/' })),
      }
      const action = keys[event.key]
      if (action) {
        event.preventDefault()
        action()
      }
    }
    document.addEventListener('keydown', onKey)
    return () => {
      document.removeEventListener('keydown', onKey)
    }
  }, [navigate, workspace])

  const header = (
    <header className="sticky top-0 z-5 flex flex-wrap items-center gap-x-3.5 gap-y-1 border-b bg-background px-3 py-2 sm:px-4 sm:py-2.5">
      <ListLink workspace={workspace} className="inline-flex items-center gap-1.5 text-lead font-semibold">
        <ArrowLeft className="size-4" aria-hidden="true" />
        <span className="sr-only">Issues</span>
        {!workspace && 'Punchlist'}
      </ListLink>
      {workspace && <WorkspaceMenu current={workspace} />}
      <span className="ml-auto text-meta text-ink-3" data-viewer={viewer.role}>
        Viewing as {viewer.role === 'agent' ? agentName(viewer.kind) : viewer.name} ({ROLE_WORD[viewer.role]})
      </span>
    </header>
  )

  if (issue.isPending || issue.isError)
    return (
      <div className="flex min-h-screen flex-col">
        {header}
        <p className={cn('p-6 text-center', issue.isError ? 'text-bad' : 'text-ink-3')} role={issue.isError ? 'alert' : undefined}>
          {issue.isError ? `Could not load ${id}: ${issue.error.message}` : `Loading ${id}…`}
        </p>
      </div>
    )

  const data = issue.data
  const moves = transitions.data
  const status = moves?.status
  const pr = pullRequests.data?.[0]
  const prView = pullRequestView(
    pr && {
      number: pr.number,
      state: pr.state === 'open' && pr.draft ? 'draft' : pr.state,
      checks: pr.checks,
      url: plainPullRequestUrl(pr.number, pr.url),
    },
  )

  return (
    <div className="flex min-h-screen flex-col" data-issue-page={id} data-viewer={viewer.role}>
      {header}
      <main className="mx-auto w-full max-w-[1280px] flex-1 px-3 py-3 sm:px-4">
        <div className="issue-page">
          <section className="panel head" data-section="head">
            <div className="grid gap-2">
              <div className="flex flex-wrap items-center gap-1.5 text-meta text-ink-3">
                <ListLink workspace={workspace} className="hover:text-foreground">
                  Issues
                </ListLink>
                <span>›</span>
                <span className="font-mono">{data.id}</span>
              </div>
              <h1 className="m-0 text-title leading-tight font-semibold wrap-anywhere">{data.title}</h1>
              <div className="flex flex-wrap items-center gap-x-3.5 gap-y-1.5 text-meta text-muted-foreground" data-props>
                <span data-status-pill={data.status}>
                  <StatusPill status={data.status} name={data.status_name} />
                </span>
                {status && <LockBadge status={status} />}
                <Assignee view={assigneeView(data.assignee)} />
                {prView.url ? (
                  <a
                    href="#pr"
                    onClick={(event) => {
                      event.preventDefault()
                      jump('pr')
                    }}
                  >
                    <PullRequest view={prView} />
                  </a>
                ) : (
                  <PullRequest view={prView} />
                )}
                {(data.labels ?? []).length > 0 && (
                  <span data-labels className="-ml-1.5">
                    <Labels labels={data.labels ?? []} />
                  </span>
                )}
                {data.milestone && <span className="text-ink-3">{data.milestone.name}</span>}
              </div>
            </div>
          </section>

          <div className="main">
            <Notice
              moves={moves}
              viewer={viewer}
              waiting={data.waiting_on_person ?? null}
              usageLimit={data.waiting_for_usage_limit ?? null}
            />
            <SpecPanel body={data.body} />
            <PullRequestPanel
              pr={pr}
              issueId={data.id}
              threads={reviewThreads.data}
              wait={waitsOnMerge(prView, status)}
            />
            <ProofPanel id={id} proof={proof.data ?? []} person={person} locked={moves?.viewer_locked ?? false} />
            <ActivityPanel
              id={id}
              events={events.data ?? []}
              feed={feed}
              onFeed={setFeed}
              viewer={viewer}
              moves={moves}
              composer={composer}
              target={target}
              now={now}
            />
          </div>

          <aside className="rail" aria-label="Status, gates and runs">
            <TransitionsPanel id={id} workspaceId={workspaceId} moves={moves} gates={gates.data} viewer={viewer} />
            <GatesPanel checklist={gates.data} />
            <RunsPanel issueId={id} runs={runs.data ?? []} moves={moves} person={person} now={now} />
            <DependenciesPanel issue={data} />
            <Panel title="Properties" className="rail-only-wide">
              <dl className="m-0 grid grid-cols-[84px_minmax(0,1fr)] gap-x-2.5 gap-y-1.5 text-meta">
                <dt className="text-ink-3">Assignee</dt>
                <dd className="m-0">
                  <Assignee view={assigneeView(data.assignee)} />
                </dd>
                <dt className="text-ink-3">Dispatch</dt>
                <dd className="m-0">
                  {status?.dispatch
                    ? agentName(status.dispatch)
                    : status?.locked_to.length
                      ? 'none · locked to people'
                      : 'none'}
                </dd>
                <dt className="text-ink-3">Project</dt>
                <dd className="m-0 wrap-anywhere">{data.project?.name ?? '—'}</dd>
                <dt className="text-ink-3">Milestone</dt>
                <dd className="m-0 wrap-anywhere">{data.milestone?.name ?? '—'}</dd>
                <dt className="text-ink-3">Parent</dt>
                <dd className="m-0 font-mono">
                  {data.parent ? (
                    <Link to="/issues/$id" params={{ id: data.parent }} className="text-primary hover:underline">
                      {data.parent}
                    </Link>
                  ) : (
                    '—'
                  )}
                </dd>
                <dt className="text-ink-3">Created</dt>
                <dd className="m-0">{age(data.created_at, now)} ago</dd>
                <dt className="text-ink-3">Updated</dt>
                <dd className="m-0">{age(data.updated_at, now)} ago</dd>
              </dl>
            </Panel>
          </aside>
        </div>
      </main>
      <footer className="sticky bottom-0 z-5 flex flex-wrap items-center gap-x-3 gap-y-1 border-t bg-card px-3 py-1.5 text-caption text-muted-foreground sm:gap-x-4 sm:px-4 sm:py-2 sm:text-meta">
        <span className="font-semibold text-foreground">Issue shortcuts</span>
        {(
          [
            ['g', 'gates'],
            ['t', 'activity'],
            ['c', 'comment'],
            ['Esc', 'back to the list'],
          ] as const
        ).map(([key, what]) => (
          <span key={key} className="inline-flex items-center gap-1.25 whitespace-nowrap">
            <kbd>{key}</kbd> {what}
          </span>
        ))}
      </footer>
    </div>
  )
}

// Unlike Properties, shown on phones too: what blocks the issue matters before its spec.
function DependenciesPanel({ issue }: { issue: Issue }) {
  const lists = dependencies(issue)
  if (lists.length === 0) return null
  return (
    <Panel title="Dependencies">
      <dl className="m-0 grid grid-cols-[84px_minmax(0,1fr)] gap-x-2.5 gap-y-1.5 text-meta" data-dependencies>
        {lists.map(({ label, issues }) => (
          <Fragment key={label}>
            <dt className="text-ink-3">{label}</dt>
            <dd className="m-0" data-linked={label === 'Blocks' ? 'blocks' : 'blocked-by'}>
              {issues.length === 0 ? (
                '—'
              ) : (
                <ul className="m-0 grid list-none gap-1 p-0">
                  {issues.map((linked) => (
                    <li key={linked.id} className="flex flex-wrap items-center gap-x-2 gap-y-0.5" data-linked-issue={linked.id}>
                      <Link to="/issues/$id" params={{ id: linked.id }} className="font-mono text-primary hover:underline">
                        {linked.id}
                      </Link>
                      <span className="inline-flex items-center gap-1.5 text-muted-foreground" data-linked-status>
                        <StatusDot status={linked.status} />
                        {linked.status_name}
                      </span>
                    </li>
                  ))}
                </ul>
              )}
            </dd>
          </Fragment>
        ))}
      </dl>
    </Panel>
  )
}

function Notice({
  moves,
  viewer,
  waiting,
  usageLimit,
}: {
  moves: TransitionList | undefined
  viewer: Actor
  waiting: string | null
  usageLimit: string | null
}) {
  const lock = <Lock className="mt-px size-3.5 flex-none" aria-hidden="true" />
  if (moves?.viewer_locked)
    return (
      <div className="notice border-[#fcd0b5] bg-bad-soft text-[#7c2d12]" data-notice="agent-locked" role="status">
        {lock}
        <div>
          <strong>Agents cannot act on this issue.</strong> {moves.status.name} is locked to people. Viewing as{' '}
          {viewer.name} ({ROLE_WORD[viewer.role]}): transitions, comments and proof are disabled until a person moves
          the issue.
        </div>
      </div>
    )
  if (moves && moves.status.locked_to.length > 0)
    return (
      <div className="notice border-warn-line bg-warn-soft text-[#7c3a06]" data-notice="locked" role="status">
        {lock}
        <div>
          <strong className="text-warn">Waiting on a person.</strong> {moves.status.name} is locked:{' '}
          {moves.status.locked_to.join(' and ')} cannot act here. No runs dispatch until the issue moves.
        </div>
      </div>
    )
  if (waiting)
    return (
      <div className="notice border-warn-line bg-warn-soft text-[#7c3a06]" data-notice="waiting" role="status">
        {lock}
        <div>
          <strong className="text-warn">Waiting on a person:</strong> {waiting}.
        </div>
      </div>
    )
  if (usageLimit)
    return (
      <div className="notice border-warn-line bg-warn-soft text-[#7c3a06]" data-notice="usage-limit" role="status">
        <Clock className="mt-px size-3.5 flex-none" aria-hidden="true" />
        <div>
          <strong className="text-warn">Usage limit:</strong> {usageLimit}.
        </div>
      </div>
    )
  if (moves?.status.closed)
    return (
      <div className="notice border-[#bfe6e0] bg-ok-soft text-[#115e59]" data-notice="done" role="status">
        <GitMerge className="mt-px size-3.5 flex-none" aria-hidden="true" />
        <div>
          <strong>{moves.status.name}.</strong> No transition leaves this status.
        </div>
      </div>
    )
  return null
}

function SpecPanel({ body }: { body: string }) {
  const blocks = useMemo(() => specBlocks(body), [body])
  const checks = blocks.filter((block) => block.kind === 'check')
  return (
    <Panel id="spec" title="Spec" right={checks.length > 0 && <span className="text-ink-3">criteria {checks.filter((c) => c.done).length} of {checks.length}</span>}>
      {blocks.length === 0 ? (
        <div className="empty">No spec yet.</div>
      ) : (
        <div className="grid gap-1.5 wrap-anywhere">
          {blocks.map((block, index) => {
            switch (block.kind) {
              case 'heading':
                return (
                  <div key={index} className="subhead mt-1.5">
                    {block.text}
                  </div>
                )
              case 'check':
                return (
                  <div key={index} className={cn('grid grid-cols-[16px_1fr] items-baseline gap-2', block.done && 'text-muted-foreground')} data-criterion={block.done ? 'done' : 'open'}>
                    <span
                      className={cn(
                        'inline-grid size-3.5 place-items-center rounded-[3px] border-[1.5px] text-[10px] text-white',
                        block.done ? 'border-ok bg-ok' : 'border-zinc-400',
                      )}
                      aria-hidden="true"
                    >
                      {block.done ? '✓' : ''}
                    </span>
                    <span>{block.text}</span>
                  </div>
                )
              case 'item':
                return (
                  <div key={index} className="grid grid-cols-[16px_1fr] gap-2">
                    <span className="text-center text-ink-3">•</span>
                    <span>{block.text}</span>
                  </div>
                )
              case 'text':
                return (
                  <p key={index} className="m-0">
                    {block.text}
                  </p>
                )
            }
          })}
        </div>
      )}
    </Panel>
  )
}

const CHECK_STATE = (run: CheckRun) =>
  run.status !== 'completed'
    ? 'pending'
    : ['success', 'neutral', 'skipped'].includes(run.conclusion ?? '')
      ? 'passing'
      : 'failing'

function PullRequestPanel({
  pr,
  issueId,
  threads,
  wait,
}: {
  pr: PullRequestInfo | undefined
  issueId: string
  threads: ReviewThread[] | undefined
  wait: WaitsOnMerge | null
}) {
  if (!pr)
    return (
      <Panel id="pr" title="Pull request">
        <div className="empty">No pull request yet.</div>
      </Panel>
    )
  const word = pr.state === 'open' && pr.draft ? 'draft' : pr.state
  const runs = pr.check_runs ?? []
  const url = plainPullRequestUrl(pr.number, pr.url)
  const name = `#${String(pr.number)} ${pr.title}`
  return (
    <Panel
      id="pr"
      title="Pull request"
      right={
        <span className="inline-flex items-center gap-2.5">
          {url && <GithubLink url={url} number={pr.number} text="Open on GitHub" data-open-on-github />}
          <span className="text-ink-3">
            #{pr.number} {word}
          </span>
        </span>
      }
    >
      <div className="grid gap-2.5">
        {wait && (
          <div
            className="flex flex-wrap items-center gap-x-2.5 gap-y-1.5 rounded-md border border-warn-line bg-warn-soft px-2.5 py-1.5 text-meta"
            data-waits-on-merge
          >
            <GitMerge className="size-3.5 flex-none text-warn" aria-hidden="true" />
            <span className="flex-[1_1_200px]">{mergeNotice(issueId, wait.to, pr.merge_state)}</span>
            {wait.url && <GithubLink url={wait.url} number={pr.number} text="Merge on GitHub" data-merge-link />}
          </div>
        )}
        <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
          {url ? (
            <a
              href={url}
              target="_blank"
              rel="noopener noreferrer"
              className="text-body font-semibold hover:underline wrap-anywhere"
            >
              {name}
            </a>
          ) : (
            <span className="text-body font-semibold wrap-anywhere">{name}</span>
          )}
        </div>
        <div className="flex flex-wrap items-center gap-x-3 gap-y-1 text-meta text-muted-foreground">
          <span className="font-mono text-caption text-ink-3 wrap-anywhere">{pr.branch}</span>
          <span>names {issueId}: {pr.issue_id === issueId ? 'yes' : 'no'}</span>
          <span>merge state {pr.merge_state}</span>
        </div>
        <div id="checks" className="scroll-mt-20" data-checks={pr.checks}>
          <div className="subhead">
            Checks · {pr.checks_passed} of {pr.checks_total} pass
          </div>
          {runs.length === 0 ? (
            <div className="text-meta text-ink-3">No check run has reported.</div>
          ) : (
            <table className="w-full border-collapse text-meta">
              <tbody>
                {runs.map((run) => {
                  const state = CHECK_STATE(run)
                  return (
                    <tr key={`${run.app ?? ''}/${run.name}`} data-check={run.name} data-check-state={state}>
                      <td className="border-t py-1 font-mono text-caption wrap-anywhere">{run.name}</td>
                      <td
                        className={cn(
                          'border-t py-1 text-right font-semibold whitespace-nowrap',
                          state === 'passing' ? 'text-ok' : state === 'failing' ? 'text-bad' : 'text-warn',
                        )}
                      >
                        {run.conclusion ?? run.status}
                      </td>
                    </tr>
                  )
                })}
              </tbody>
            </table>
          )}
        </div>
        <ReviewThreads issueId={issueId} threads={threads} fallback={pr.open_threads} />
      </div>
    </Panel>
  )
}

/** The open review threads of the issue's open pull requests, as recorded. Their text is GitHub users': plain text only. */
function ReviewThreads({
  issueId,
  threads,
  fallback,
}: {
  issueId: string
  threads: ReviewThread[] | undefined
  fallback: number
}) {
  const read = useReadFromGithub(issueId)
  return (
    <div id="threads" className="scroll-mt-20 text-meta">
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
        <div className="subhead">Review threads · {threads?.length ?? fallback} open</div>
        <Button
          variant="outline"
          size="xs"
          className="ml-auto"
          disabled={read.isPending}
          onClick={() => {
            read.mutate()
          }}
        >
          {read.isPending ? 'Reading…' : 'Read from GitHub'}
        </Button>
      </div>
      {!read.isPending && read.data?.refreshed === false && (
        <div className="mb-1.5 text-warn" role="status">
          GitHub could not be read. Showing what is recorded.
        </div>
      )}
      {threads?.length === 0 ? (
        <div className="text-ink-3">No open review threads.</div>
      ) : (
        <div className="grid gap-1.5">
          {threads?.map((thread) => {
            const view = threadView(thread)
            return (
              <div key={thread.id} data-thread={thread.id} className="grid gap-0.5 border-l-3 border-l-bad pl-2">
                <div className="flex flex-wrap gap-x-2 text-caption text-ink-3">
                  <span className="font-semibold text-bad">open</span>
                  <span>#{thread.pull_request}</span>
                  <span className="font-mono wrap-anywhere">{view.place}</span>
                </div>
                {view.unread && <div className="text-ink-3">Text not read yet</div>}
                {view.note && <div className="text-caption text-ink-3">{view.note}</div>}
                {view.comments.map((comment, index) => (
                  <div key={index} data-thread-comment className="whitespace-pre-wrap wrap-anywhere">
                    <span className="font-semibold">{comment.author}</span> {comment.body}
                    {comment.cut && <span className="text-ink-3"> … cut, longer on GitHub</span>}
                  </div>
                ))}
                {view.url && (
                  <a href={view.url} target="_blank" rel="noreferrer" className="text-caption hover:underline">
                    View on GitHub
                  </a>
                )}
              </div>
            )
          })}
        </div>
      )}
    </div>
  )
}

const PROOF_ACCEPT = Object.keys(PROOF_TYPES)
  .map((ext) => `.${ext}`)
  .join(',')

function ProofPanel({ id, proof, person, locked }: { id: string; proof: Proof[]; person: boolean; locked: boolean }) {
  const attach = useAttachProof(id)
  const input = useRef<HTMLInputElement>(null)
  const [open, setOpen] = useState<string | null>(null)
  const shown = proof.find((item) => item.id === open)
  return (
    <Panel
      id="proof"
      title={
        <>
          Proof <span className="font-medium tracking-normal text-ink-3 normal-case">{proof.length}</span>
        </>
      }
      right={
        !locked && (
          <>
            <input
              ref={input}
              type="file"
              accept={PROOF_ACCEPT}
              className="sr-only"
              aria-label="Attach proof"
              data-proof-input
              onChange={(event) => {
                const file = event.currentTarget.files?.[0]
                if (file) attach.mutate(file)
                event.currentTarget.value = ''
              }}
            />
            <Button
              variant="outline"
              size="xs"
              disabled={attach.isPending}
              onClick={() => input.current?.click()}
            >
              <Paperclip /> {attach.isPending ? 'Attaching…' : 'Attach file'}
            </Button>
          </>
        )
      }
    >
      {proof.length === 0 ? (
        <div className="empty" data-proof-empty>
          No proof yet. A run attaches screenshots, video and reports before it asks for review.
        </div>
      ) : (
        <div className="grid grid-cols-[repeat(auto-fill,minmax(160px,1fr))] gap-2.5">
          {proof.map((item) => (
            <ProofCard key={item.id} proof={item} person={person} onOpen={setOpen} />
          ))}
        </div>
      )}
      {shown && (
        <ProofViewer
          proof={shown}
          onClose={() => {
            setOpen(null)
          }}
          onStep={(delta) => {
            setOpen((at) => (at && mediaStep(proof, at, delta)) ?? at)
          }}
        />
      )}
    </Panel>
  )
}

const proofMeta = (proof: Proof) => `${proof.kind} · ${fileSize(proof.size)} · ${proof.actor.name}`

function ProofCard({ proof, person, onOpen }: { proof: Proof; person: boolean; onOpen: (id: string) => void }) {
  const src = `/api/proof/${proof.id}`
  // An agent's token cannot read the files themselves.
  const thumb =
    proof.kind !== 'report' && person ? (
      <button
        type="button"
        className="block cursor-zoom-in focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none focus-visible:ring-inset"
        aria-label={`Open ${proof.title}`}
        data-proof-open
        onClick={() => {
          onOpen(proof.id)
        }}
      >
        {proof.kind === 'image' ? (
          <img src={src} alt="" loading="lazy" className="aspect-[16/10] w-full bg-secondary object-cover" />
        ) : (
          <video src={src} preload="metadata" muted className="pointer-events-none aspect-[16/10] w-full bg-zinc-900" />
        )}
      </button>
    ) : proof.kind === 'report' ? null : (
      <div className="aspect-[16/10] w-full bg-gradient-to-br from-primary-line to-primary" aria-hidden="true" />
    )
  return (
    <article className="grid overflow-hidden rounded-md border bg-card" data-proof={proof.kind} data-proof-id={proof.id}>
      {thumb}
      <div className="grid gap-0.5 px-2.5 py-2">
        {person ? (
          <a href={src} target="_blank" rel="noreferrer" className="text-meta font-medium wrap-anywhere hover:underline">
            {proof.title}
          </a>
        ) : (
          <span className="text-meta font-medium wrap-anywhere">{proof.title}</span>
        )}
        <span className="text-caption text-ink-3">{proofMeta(proof)}</span>
      </div>
    </article>
  )
}

/** An image or video proof full size, scaled down to fit the viewport; `←` and `→` step through the others. */
function ProofViewer({
  proof,
  onClose,
  onStep,
}: {
  proof: Proof
  onClose: () => void
  onStep: (delta: 1 | -1) => void
}) {
  const src = `/api/proof/${proof.id}`
  const media = 'max-h-[calc(100dvh-9rem)] max-w-full object-contain'
  // The card that opened it; Radix would return focus to a `DialogTrigger`, and the cards are not one.
  const [opener] = useState(() => document.activeElement)
  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open) onClose()
      }}
    >
      <DialogContent
        className="w-max max-w-[calc(100vw-2rem)] gap-2 bg-card p-3 sm:max-w-[calc(100vw-2rem)]"
        data-proof-viewer={proof.id}
        onCloseAutoFocus={(event) => {
          event.preventDefault()
          if (opener instanceof HTMLElement) opener.focus()
        }}
        // The page's own keys (`Esc` back to the list, `g`, `t`, `c`) stay out of the overlay.
        onKeyDown={(event) => {
          event.stopPropagation()
          const delta = event.key === 'ArrowLeft' ? -1 : event.key === 'ArrowRight' ? 1 : null
          if (delta === null) return
          event.preventDefault()
          onStep(delta)
        }}
      >
        <div className="flex min-w-0 flex-wrap items-baseline gap-x-3 gap-y-0.5 pr-8">
          <DialogTitle className="text-meta font-medium wrap-anywhere">{proof.title}</DialogTitle>
          <DialogDescription className="text-caption text-ink-3">{proofMeta(proof)}</DialogDescription>
          <a
            href={src}
            target="_blank"
            rel="noreferrer"
            className="ml-auto text-caption font-medium text-primary hover:underline"
            data-proof-original
          >
            Open original
          </a>
        </div>
        {proof.kind === 'image' ? (
          <img key={proof.id} src={src} alt={proof.title} className={media} />
        ) : (
          <video key={proof.id} src={src} controls autoPlay className={media} aria-label={proof.title} />
        )}
      </DialogContent>
    </Dialog>
  )
}

const FEEDS: [Feed, string][] = [
  ['all', 'All'],
  ['comments', 'Comments'],
  ['transitions', 'Transitions'],
]

function ActivityPanel({
  id,
  events,
  feed,
  onFeed,
  viewer,
  moves,
  composer,
  target,
  now,
}: {
  id: string
  events: IssueEvent[]
  feed: Feed
  onFeed: (feed: Feed) => void
  viewer: Actor
  moves: TransitionList | undefined
  composer: RefObject<HTMLTextAreaElement | null>
  target: string
  now: Date
}) {
  const shown = useMemo(() => feedEntries(events, feed), [events, feed])
  const comments = useMemo(() => events.filter((event) => event.detail.kind === 'comment').length, [events])
  const post = useComment(id)
  const [text, setText] = useState('')
  const locked = moves?.viewer_locked ?? false
  return (
    <Panel
      id="timeline"
      title={
        <>
          Activity <span className="font-medium tracking-normal text-ink-3 normal-case">{events.length}</span>
        </>
      }
      right={
        <span className="feed-filter" role="group" aria-label="Activity filter">
          {FEEDS.map(([key, label]) => (
            <button key={key} type="button" data-feed={key} aria-pressed={feed === key} onClick={() => {
                onFeed(key)
              }}
            >
              {label}
              {key === 'comments' && <span className="opacity-80">{comments}</span>}
            </button>
          ))}
        </span>
      }
    >
      {shown.length === 0 ? (
        <div className="empty">No {feed === 'all' ? 'activity' : feed} yet.</div>
      ) : (
        <Timeline events={shown} target={target} now={now} />
      )}
      <form
        className="composer"
        data-composer
        data-locked={locked}
        onSubmit={(event) => {
          event.preventDefault()
          const body = text.trim()
          if (!body || locked) return
          post.mutate(body, {
            onSuccess: () => {
              setText('')
              if (feed === 'transitions') onFeed('all')
            },
          })
        }}
      >
        <textarea
          ref={composer}
          name="body"
          rows={2}
          value={text}
          disabled={locked}
          aria-label="New comment"
          placeholder={locked ? `Commenting is disabled for ${ROLE_WORD[viewer.role]}s in ${moves?.status.name ?? 'this status'}` : 'Leave a comment…'}
          onChange={(event) => {
            setText(event.target.value)
          }}
          onKeyDown={(event) => {
            if (event.key === 'Enter' && (event.metaKey || event.ctrlKey)) event.currentTarget.form?.requestSubmit()
          }}
        />
        <div className="flex flex-wrap items-center justify-between gap-2.5 text-caption">
          {locked ? (
            <span className="inline-flex items-center gap-1.25 text-warn" data-composer-note>
              <Lock className="size-2.75" aria-hidden="true" /> {moves?.status.name} is locked to people. {viewer.name}{' '}
              cannot comment here.
            </span>
          ) : (
            <span className="text-ink-3">
              as {viewer.name}
              {viewer.role === 'agent' && ' · ends with (agent)'}
            </span>
          )}
          <Button type="submit" size="xs" disabled={locked || post.isPending || !text.trim()}>
            Comment
          </Button>
        </div>
      </form>
    </Panel>
  )
}

const Timeline = memo(function Timeline({ events, target, now }: { events: IssueEvent[]; target: string; now: Date }) {
  return (
    <ol className="timeline">
      {events.map((event) => (
        <Entry key={event.seq} event={event} targeted={target === `seq-${String(event.seq)}`} now={now} />
      ))}
    </ol>
  )
})

const Entry = memo(function Entry({ event, targeted, now }: { event: IssueEvent; targeted: boolean; now: Date }) {
  const mark = actorMark(event)
  const { detail } = event
  const at = (
    <time className="text-caption leading-5 whitespace-nowrap text-ink-3" dateTime={event.created_at} title={event.created_at}>
      {age(event.created_at, now)}
    </time>
  )
  const who = (
    <>
      <span className="font-semibold text-foreground">{mark.name}</span>
      <Chip mark={mark} />
    </>
  )
  return (
    <li
      id={`seq-${String(event.seq)}`}
      className="relative scroll-mt-20 py-1.25"
      data-kind={detail.kind}
      data-actor-type={mark.mark}
      data-seq={event.seq}
      data-target={targeted || undefined}
    >
      <div className="grid grid-cols-[20px_minmax(0,1fr)_auto] items-start gap-2 text-meta text-muted-foreground">
        <Avatar mark={mark} />
        <span className="flex min-h-5 flex-wrap items-center gap-x-1.5 gap-y-1">
          {detail.kind === 'refusal' ? (
            <>
              {who}
              <span className="text-bad wrap-anywhere">
                refused {event.actor.name}&apos;s move to {detail.to_name}: <b className="font-semibold">{detail.gate}</b> ·{' '}
                {detail.reason}
              </span>
            </>
          ) : detail.kind === 'transition' ? (
            <>
              {who}
              <span className="text-ink-3">moved</span>
              <span className="inline-flex items-center gap-1.25 font-medium text-foreground">
                <StatusDot status={detail.from} />
                {detail.from_name}
                <span className="font-normal text-ink-3">→</span>
                <StatusDot status={detail.to} />
                {detail.to_name}
              </span>
              {detail.delivery_id && <span className="text-ink-3">on a GitHub event</span>}
            </>
          ) : detail.kind === 'comment' ? (
            <>
              {who}
              <span className="text-ink-3">commented</span>
            </>
          ) : (
            <>
              {who}
              <span className="text-ink-3">{eventText(event)}</span>
            </>
          )}
        </span>
        {at}
      </div>
      {detail.kind === 'comment' && (
        <div
          className={cn(
            'mt-1.5 ml-7.5 rounded-md border bg-card px-2.5 py-2 text-body whitespace-pre-line text-foreground wrap-anywhere',
            mark.mark === 'agent' && 'border-agent-line',
          )}
          data-comment
        >
          {detail.body}
        </div>
      )}
    </li>
  )
})

function TransitionsPanel({
  id,
  workspaceId,
  moves,
  gates,
  viewer,
}: {
  id: string
  workspaceId: string
  moves: TransitionList | undefined
  gates: GateChecklist | null | undefined
  viewer: Actor
}) {
  const move = useMove(id, workspaceId)
  const views = (moves?.transitions ?? []).flatMap((option) => {
    const view = moveView(option, moves?.viewer_locked ?? false, gates)
    return view ? [{ view, option }] : []
  })
  return (
    <Panel
      id="transitions"
      title="Status"
      right={moves && <StatusPill status={moves.status.key} name={moves.status.name} />}
    >
      <div className="mb-2 text-caption text-muted-foreground">
        Moves for {viewer.name} ({ROLE_WORD[viewer.role]})
      </div>
      <div className="grid gap-1.5">
        {!moves ? (
          <div className="text-meta text-ink-3">Loading…</div>
        ) : views.length === 0 ? (
          <div className="text-meta text-ink-3 italic" data-no-moves>
            {moves.status.closed
              ? `No further moves. ${moves.status.name} is terminal.`
              : `No move from ${moves.status.name} is ${viewer.role === 'person' ? 'a person' : `an ${ROLE_WORD[viewer.role]}`}'s to make.`}
          </div>
        ) : (
          views.map(({ view, option }) => (
            <button
              key={view.to}
              type="button"
              className={cn('transition', view.state)}
              data-transition={view.to}
              data-verdict={view.state}
              aria-disabled={view.disabled || move.isPending}
              onClick={() => {
                if (view.disabled) toast(`${view.toName}: ${view.gate ? `${view.gate} · ` : ''}${view.text}`)
                else if (!move.isPending) move.mutate(option.to)
              }}
            >
              <span className="text-meta text-ink-3">→</span>
              <span className="inline-flex flex-wrap items-center gap-1.5 font-semibold">
                <StatusDot status={view.to} />
                <span className="whitespace-nowrap">{view.toName}</span>
                <LockBadge status={option.to} />
              </span>
              <span className="text-caption whitespace-nowrap text-ink-3">{view.by}</span>
              <span className="verdict">
                <span className="flex-none font-bold">
                  {view.state === 'allowed' ? '✓' : view.state === 'blocked' ? '✕' : <Lock className="mt-0.5 size-3" aria-label="locked" />}
                </span>
                {view.state === 'blocked' ? (
                  <span className="min-w-0 wrap-anywhere">
                    <b className="font-mono font-semibold" data-blocked-gate={view.gate}>
                      {view.gate}
                    </b>
                    <span className="text-muted-foreground">
                      {' '}
                      · {view.text}
                      {view.names.length > 0 && (
                        <>
                          : <span className="font-mono" data-blocked-names>{view.names.join(', ')}</span>
                        </>
                      )}
                    </span>
                  </span>
                ) : (
                  <span className="min-w-0 wrap-anywhere">{view.text}</span>
                )}
              </span>
            </button>
          ))
        )}
      </div>
    </Panel>
  )
}

function GatesPanel({ checklist }: { checklist: GateChecklist | null | undefined }) {
  const summary = gateSummary(checklist)
  return (
    <Panel
      id="gates"
      title={summary.toName ? `Gates to enter ${summary.toName}` : 'Gates'}
      right={summary.total > 0 && <span className="text-ink-3" data-gate-summary>{summary.text}</span>}
    >
      {!checklist ? (
        <div className="text-meta text-ink-3">No gates on the next move.</div>
      ) : (
        <>
          <div className="grid gap-1.5">
            {checklist.gates.map((result) => {
              const evidence = GATE_EVIDENCE[result.gate] ?? 'pr'
              return (
                <div
                  key={result.gate}
                  className="grid grid-cols-[18px_minmax(0,1fr)] items-baseline gap-x-2 gap-y-0.5 text-meta"
                  data-gate={result.gate}
                  data-pass={result.passed}
                >
                  <span className={cn('text-center font-bold', result.passed ? 'text-ok' : 'text-bad')}>
                    {result.passed ? '✓' : '✕'}
                  </span>
                  <span className={cn('font-mono font-semibold', result.passed ? 'text-foreground' : 'text-bad')}>
                    {result.gate}
                  </span>
                  <span className="col-start-2 text-muted-foreground wrap-anywhere">{gateText(result)}</span>
                  <a
                    href={`#${evidence}`}
                    className="col-start-2 text-caption text-primary hover:underline"
                    data-evidence={evidence}
                    onClick={(event) => {
                      event.preventDefault()
                      jump(evidence)
                    }}
                  >
                    evidence
                  </a>
                </div>
              )
            })}
          </div>
          <div className="mt-2.5 flex items-center gap-2 text-meta">
            <span className="inline-flex gap-0.5" aria-hidden="true">
              {checklist.gates.map((result) => (
                <b key={result.gate} className={cn('inline-block h-1.5 w-3 rounded-[2px]', result.passed ? 'bg-ok' : 'bg-bad')} />
              ))}
            </span>
            <span className={summary.passed === summary.total ? 'text-ok' : 'text-muted-foreground'}>{summary.text}</span>
          </div>
        </>
      )}
    </Panel>
  )
}

const OUTCOME: Record<AttemptInfo['state'], string> = {
  running: 'border-[#bfe6e0] bg-ok-soft text-ok',
  succeeded: 'border-[#bfe6e0] bg-ok-soft text-ok',
  failed: 'border-[#fcd0b5] bg-bad-soft text-bad',
}

function RunsPanel({
  issueId,
  runs,
  moves,
  person,
  now,
}: {
  issueId: string
  runs: Run[]
  moves: TransitionList | undefined
  person: boolean
  now: Date
}) {
  const status = moves?.status
  const count = runs.reduce((n, run) => n + (run.attempts ?? []).length, 0)
  return (
    <Panel
      id="runs"
      title={
        <>
          Runs <span className="font-medium tracking-normal text-ink-3 normal-case">{count}</span>
        </>
      }
      right={
        status?.locked_to.length ? (
          <span className="inline-flex items-center gap-1 text-warn">
            <Lock className="size-2.5" aria-hidden="true" /> no dispatch
          </span>
        ) : (
          status?.dispatch && <span className="text-ink-3">dispatches {agentName(status.dispatch)}</span>
        )
      }
    >
      {runs.length === 0 ? (
        <div className="text-meta text-ink-3">No runs yet.</div>
      ) : (
        <div className="grid gap-2">
          {runs.flatMap((run) =>
            [...(run.attempts ?? [])].reverse().map((attempt) => (
              <AttemptCard
                key={`${run.id}/${String(attempt.attempt)}`}
                issueId={issueId}
                run={run}
                attempt={attempt}
                latest={attempt.attempt === run.attempt}
                person={person}
                now={now}
              />
            )),
          )}
        </div>
      )}
    </Panel>
  )
}

function AttemptCard({
  issueId,
  run,
  attempt,
  latest,
  person,
  now,
}: {
  issueId: string
  run: Run
  attempt: AttemptInfo
  latest: boolean
  person: boolean
  now: Date
}) {
  const running = attempt.state === 'running'
  const log = useQuery({ ...runLogQuery(issueId, run.id, running), enabled: person && latest })
  const all = log.data ?? run.log_tail
  const lines = useMemo(
    () => all.filter((line: LogLine) => line.attempt === attempt.attempt).map((line) => ({ ...line, view: logView(line.line) })),
    [all, attempt.attempt],
  )
  const box = useRef<HTMLOListElement>(null)
  // Follows the newest line until the person scrolls up, and again once they scroll back to the bottom.
  const follow = useRef(true)
  useLayoutEffect(() => {
    if (follow.current && box.current) box.current.scrollTop = box.current.scrollHeight
  }, [lines.length])
  const model = modelView(attempt)
  const took = running ? now.getTime() - Date.parse(attempt.started_at) : attempt.duration_ms
  const spent = cost(attempt.cost_usd)
  return (
    <article
      className="grid gap-2 rounded-md border px-3 py-2.5"
      data-run={run.id}
      data-attempt={attempt.attempt}
      data-outcome={attempt.state}
    >
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1.5 text-meta text-muted-foreground">
        <span className="font-mono text-ink-3">{run.id.slice(0, 8)}</span>
        <span>attempt {attempt.attempt}</span>
        <Assignee view={{ kind: 'agent', label: run.agent_name, name: run.agent_name }} />
        <span>
          on <span className="font-medium text-foreground">{attempt.runner.name}</span>
        </span>
        <span
          className={cn(
            'inline-flex items-center gap-1.25 rounded-full border px-2 py-px text-caption font-semibold',
            OUTCOME[attempt.state],
          )}
        >
          <i className={cn('inline-block size-1.75 rounded-full bg-current', running && 'run-pulse')} aria-hidden="true" />
          {attempt.state}
        </span>
      </div>
      <div className="flex flex-wrap items-baseline gap-x-2 text-body" data-model>
        <b className="text-meta font-medium text-ink-3">model</b>
        {model.name === null ? (
          <span className="text-muted-foreground">{model.note}</span>
        ) : (
          <>
            <span className="font-mono font-medium text-foreground wrap-anywhere">{model.name}</span>
            <span className="text-meta text-muted-foreground">({model.note})</span>
          </>
        )}
      </div>
      <div className="flex flex-wrap gap-x-3.5 gap-y-1 text-meta text-muted-foreground" data-facts>
        <span>
          <b className="font-medium text-ink-3">started</b> {age(attempt.started_at, now)} ago
        </span>
        <span>
          <b className="font-medium text-ink-3">duration</b> {duration(took)}
        </span>
        <span>
          <b className="font-medium text-ink-3">tokens</b> {tokens(attempt.input_tokens, attempt.output_tokens)}
        </span>
        {spent !== null && (
          <span>
            <b className="font-medium text-ink-3">cost</b> {spent}
          </span>
        )}
      </div>
      {attempt.failure_reason && <div className="text-meta text-bad wrap-anywhere">{attempt.failure_reason}</div>}
      {latest && lines.length > 0 && (
        <ol
          ref={box}
          className={cn('run-log', running && 'live')}
          data-log={running ? 'live' : 'done'}
          aria-live={running ? 'polite' : undefined}
          onScroll={(e) => {
            const el = e.currentTarget
            follow.current = el.scrollHeight - el.scrollTop - el.clientHeight < 24
          }}
        >
          {lines.map(({ seq, line, view }) => (
            <li key={seq} data-line-kind={view.kind}>
              {view.kind === 'plain' ? (
                view.text
              ) : (
                <details>
                  <summary>{view.text}</summary>
                  <pre data-raw>{line}</pre>
                </details>
              )}
            </li>
          ))}
        </ol>
      )}
    </article>
  )
}
