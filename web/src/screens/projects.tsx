import { useCallback, useEffect, useLayoutEffect, useState } from 'react'
import { Link, useNavigate } from '@tanstack/react-router'
import { cn } from 'cn'
import { ApiError } from '@/api/client'
import { AccountMenu } from '@/components/account-menu'
import { AppHeader, inField, useScreenActions } from '@/components/app-header'
import { ShortcutStrip, focusNext, focused } from '@/components/shortcut-strip'
import { STRIPS } from '@/model/shortcuts'
import { StatusDot } from '@/components/atoms'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogTitle } from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import {
  useEventStream,
  useInbox,
  useIssues,
  useProjects,
  usePullRequests,
  useSaveProject,
  useWorkflow,
} from '@/lib/queries'
import { milestoneProgress, type Milestone, type Progress, type Project } from '@/model/projects'
import type { Workspace } from '@/model/workspaces'

const plural = (n: number, word: string) => `${String(n)} ${word}${n === 1 ? '' : 's'}`

/** The form open on the screen: a new project or milestone, or an edit of one. */
type Form = { kind: 'project'; project?: Project } | { kind: 'milestone'; project: Project; milestone?: Milestone }

/** What the strip's `Tab` and `Enter` walk and open: the milestones and their issues. */
const ITEMS = '[data-milestone] button[aria-expanded], [data-issue-link]'

const fieldClass =
  'rounded-md border border-input bg-transparent px-3 py-2 shadow-xs outline-none focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring/50'

/**
 * Creates or edits a project or a milestone. It stays open on a refusal, showing the server's message, and closes
 * once the server has saved it. `Esc` closes it.
 */
function ProjectForm({ workspaceId, form, onClose }: { workspaceId: string; form: Form; onClose: () => void }) {
  const save = useSaveProject(workspaceId)
  const editing = form.kind === 'project' ? form.project : form.milestone
  const oldPassWhen = form.kind === 'milestone' ? (form.milestone?.pass_when ?? '') : ''
  const [name, setName] = useState(editing?.name ?? '')
  const [passWhen, setPassWhen] = useState(oldPassWhen)
  const title = editing
    ? form.kind === 'project'
      ? 'Rename project'
      : 'Edit milestone'
    : form.kind === 'project'
      ? 'New project'
      : `New milestone in ${form.project.name}`
  const error = save.error && (save.error instanceof ApiError ? save.error.body.message : String(save.error))
  return (
    <DialogContent className="bg-card" data-form={form.kind}>
      <DialogTitle className="text-title wrap-anywhere">{title}</DialogTitle>
      <DialogDescription className="sr-only">
        {form.kind === 'project' ? 'A project groups milestones.' : 'A milestone groups issues toward one outcome.'}
      </DialogDescription>
      <form
        className="grid gap-2"
        noValidate
        onSubmit={(event) => {
          event.preventDefault()
          save.mutate(
            form.kind === 'project'
              ? { kind: 'project', id: form.project?.id, name }
              : {
                  kind: 'milestone',
                  project: form.project.id,
                  id: form.milestone?.id,
                  name,
                  // An edit sends PASS when only when it changed, since a sent one is kept over Linear's.
                  pass_when: form.milestone && passWhen === oldPassWhen ? undefined : passWhen,
                },
            { onSuccess: onClose },
          )
        }}
      >
        <label htmlFor="form-name" className="text-meta font-semibold">
          Name
        </label>
        <Input
          id="form-name"
          autoComplete="off"
          autoFocus
          value={name}
          aria-invalid={error ? true : undefined}
          aria-describedby={error ? 'form-hint form-error' : 'form-hint'}
          onChange={(event) => {
            setName(event.target.value)
          }}
        />
        <p id="form-hint" className="text-caption text-muted-foreground">
          1 to 100 characters, unique in {form.kind === 'project' ? 'the workspace' : form.project.name}.
        </p>
        {error && (
          <p id="form-error" className="text-meta text-bad wrap-anywhere" role="alert" data-form-error>
            {error}
          </p>
        )}
        {form.kind === 'milestone' && (
          <>
            <label htmlFor="form-pass-when" className="mt-1 text-meta font-semibold">
              PASS when
            </label>
            <textarea
              id="form-pass-when"
              rows={3}
              value={passWhen}
              aria-describedby="form-pass-when-hint"
              className={fieldClass}
              onChange={(event) => {
                setPassWhen(event.target.value)
              }}
            />
            <p id="form-pass-when-hint" className="text-caption text-muted-foreground">
              What has to be true for the milestone to pass. Optional.
            </p>
          </>
        )}
        <DialogFooter className="mt-2">
          <Button type="button" variant="outline" onClick={onClose}>
            Cancel
          </Button>
          <Button type="submit" disabled={save.isPending}>
            {save.isPending ? 'Saving…' : editing ? 'Save' : `Create ${form.kind}`}
          </Button>
        </DialogFooter>
      </form>
    </DialogContent>
  )
}

/** Records a `pl:render` measure for each commit of the screen with its milestones, as the list does. */
function useRenderMeasure(milestones: number | undefined) {
  const start = 'pl:render-start:projects'
  performance.mark(start)
  useLayoutEffect(() => {
    if (milestones !== undefined) performance.measure('pl:render', { start, detail: { layout: 'projects', milestones } })
  })
}

/** `n of m done`, the canceled issues left out, and a bar split by status in the workflow's order. */
function ProgressBlock({ milestone, progress, prefix }: { milestone: Milestone; progress: Progress; prefix: string }) {
  const { done, total, canceled, byStatus } = progress
  return (
    <div className="grid content-start gap-1" data-progress>
      <div className="flex flex-wrap items-baseline gap-x-2 gap-y-0.5 text-meta">
        <span className="font-semibold" data-done-count>
          {done} of {total} done
        </span>
        {canceled > 0 && (
          <span className="text-ink-3" data-canceled>
            {canceled} canceled, not counted
          </span>
        )}
      </div>
      <div
        className={cn(
          'flex h-2 gap-px overflow-hidden rounded-sm',
          total === 0 ? 'border border-dashed' : 'bg-border',
        )}
        data-bar
      >
        {byStatus.map((status) => (
          <span
            key={status.key}
            role="img"
            aria-label={`${status.name}: ${String(status.count)}`}
            title={`${status.name}: ${String(status.count)}`}
            data-status={status.key}
            data-count={status.count}
            className="basis-0 bg-[var(--s-fill,var(--s-color,var(--color-ink-3)))]"
            style={{ flexGrow: status.count }}
          />
        ))}
      </div>
      <div className="flex flex-wrap items-center gap-x-2.5 gap-y-0.5 text-caption text-muted-foreground">
        {byStatus.map((status) => (
          <span key={status.key} className="inline-flex items-center gap-1 whitespace-nowrap" aria-hidden="true">
            <StatusDot status={status.key} />
            {status.count} {status.name}
          </span>
        ))}
        <Link
          to="/w/$prefix"
          params={{ prefix }}
          search={{ milestone: milestone.id }}
          className="ml-auto rounded-sm font-medium whitespace-nowrap text-primary hover:underline focus-visible:outline-2 focus-visible:outline-ring"
          data-show-in-list
        >
          Show in list<span className="sr-only"> {milestone.name}</span>
        </Link>
      </div>
    </div>
  )
}

function MilestoneRow({
  milestone,
  progress,
  prefix,
  open,
  onToggle,
  onEdit,
}: {
  milestone: Milestone
  progress: Progress
  prefix: string
  open: boolean
  onToggle: () => void
  onEdit: () => void
}) {
  const passWhen = milestone.pass_when.trim()
  return (
    <article
      className={cn(
        'grid grid-cols-1 items-start gap-x-4 gap-y-2 border-t px-3 py-2.5 first:border-t-0 md:grid-cols-[minmax(0,1fr)_260px]',
        open && 'bg-background',
      )}
      data-milestone={milestone.id}
    >
      <div className="grid gap-1">
        <div className="flex items-start justify-between gap-2">
          <button
            type="button"
            className="group cursor-pointer rounded-sm text-left font-semibold wrap-anywhere focus-visible:outline-2 focus-visible:outline-ring"
            aria-expanded={open}
            aria-controls={`issues-${milestone.id}`}
            onClick={onToggle}
          >
            <span className="text-ink-3" aria-hidden="true">
              {open ? '▾ ' : '▸ '}
            </span>
            <span className="group-hover:text-primary group-hover:underline" data-name>
              {milestone.name}
            </span>
          </button>
          <Button type="button" variant="ghost" size="xs" onClick={onEdit} data-edit-milestone={milestone.id}>
            Edit<span className="sr-only"> milestone {milestone.name}</span>
          </Button>
        </div>
        <div className="grid gap-0.5 text-meta text-muted-foreground" data-pass-when>
          <span className="text-caption font-semibold tracking-[.04em] text-ink-3">PASS when</span>
          {passWhen ? (
            <span className="leading-normal whitespace-pre-line wrap-anywhere">{milestone.pass_when}</span>
          ) : (
            <span className="text-ink-3 italic">No PASS when yet</span>
          )}
        </div>
      </div>
      <ProgressBlock milestone={milestone} progress={progress} prefix={prefix} />
      {open && (
        <div id={`issues-${milestone.id}`} className="col-span-full grid gap-1.5" data-issues>
          <h4 className="text-meta font-semibold text-muted-foreground">{plural(progress.issues.length, 'issue')}</h4>
          {progress.issues.length === 0 ? (
            <p className="text-meta text-ink-3">No issues in this milestone yet. Pick it in an issue&apos;s sheet.</p>
          ) : (
            <ul className="rounded-md border bg-card">
              {progress.issues.map(({ issue }) => (
                <li
                  key={issue.id}
                  className="flex flex-wrap items-center justify-between gap-x-3 gap-y-0.5 border-t px-2.5 py-1.5 first:border-t-0"
                >
                  <Link
                    to="/issues/$id"
                    params={{ id: issue.id }}
                    className="flex flex-[1_1_220px] items-baseline gap-2 rounded-sm hover:underline focus-visible:outline-2 focus-visible:outline-ring"
                    data-issue-link={issue.id}
                  >
                    <span className="flex-none font-mono text-meta whitespace-nowrap text-ink-3">{issue.id}</span>
                    <span className="wrap-anywhere">{issue.title}</span>
                  </Link>
                  <span className="inline-flex items-center gap-1.25 text-meta whitespace-nowrap text-muted-foreground">
                    <StatusDot status={issue.status} />
                    {issue.status_name}
                  </span>
                </li>
              ))}
            </ul>
          )}
        </div>
      )}
    </article>
  )
}

/**
 * Each project, its milestones with their PASS when and progress, and a milestone's issues once opened. Progress
 * comes from the cached list, which the event stream keeps current.
 */
export function ProjectsScreen({ workspace, me }: { workspace: Workspace; me: string }) {
  const issues = useIssues(workspace.id)
  useEventStream(workspace.id, issues.isSuccess)
  const pullRequests = usePullRequests(workspace.id, issues.isSuccess)
  const inbox = useInbox(workspace.id, issues.isSuccess)
  const projects = useProjects(workspace.id)
  const workflow = useWorkflow(workspace.id)
  const navigate = useNavigate()
  const screenActions = useScreenActions(workspace.issue_prefix)
  const [open, setOpen] = useState<string | null>(null)
  const [form, setForm] = useState<Form | null>(null)
  const ready = projects.data && issues.data && workflow.data
  const milestones = projects.data?.reduce((n, p) => n + p.milestones.length, 0) ?? 0
  useRenderMeasure(ready ? milestones : undefined)

  const toList = useCallback(
    () => void navigate({ to: '/w/$prefix', params: { prefix: workspace.issue_prefix } }),
    [navigate, workspace.issue_prefix],
  )
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== 'Escape' || inField(event.target)) return
      // Radix closes its dialog or menu on this Esc and marks it handled, and the dialog can already be gone here.
      if (event.defaultPrevented || document.querySelector('[role="dialog"], [role="menu"]')) return
      event.preventDefault()
      toList()
    }
    document.addEventListener('keydown', onKey)
    return () => {
      document.removeEventListener('keydown', onKey)
    }
  }, [toList])

  const failed = projects.error ?? issues.error ?? workflow.error

  return (
    <div className="flex min-h-screen flex-col">
      <AppHeader
        workspace={workspace}
        current="projects"
        pullRequests={pullRequests.data?.length}
        inbox={inbox.data?.unread}
      >
        <AccountMenu me={me} className="ml-auto" />
      </AppHeader>

      <main className="mx-auto grid w-full max-w-320 flex-1 content-start gap-3 px-3 py-2 sm:px-4 sm:py-3" aria-live="polite">
        <div className="flex flex-wrap items-center justify-between gap-2">
          <h1 className="flex flex-wrap items-baseline gap-x-2.5 gap-y-1 text-title font-semibold">
            Projects
            {projects.data && (
              <span className="text-meta font-normal text-ink-3">
                {plural(projects.data.length, 'project')} · {plural(milestones, 'milestone')}
              </span>
            )}
          </h1>
          <Button
            type="button"
            size="sm"
            data-new-project
            onClick={() => {
              setForm({ kind: 'project' })
            }}
          >
            + New project
          </Button>
        </div>
        <p className="text-meta text-muted-foreground">
          Progress counts a milestone&apos;s issues in <b>Done</b> out of its issues. <b>Canceled</b> issues are left
          out of the total. The bar is split by status, in the workflow&apos;s order.
        </p>
        {failed ? (
          <p className="p-6 text-center text-bad" role="alert">
            Could not load the projects: {String(failed)}
          </p>
        ) : !ready ? (
          <p className="p-6 text-center text-ink-3">Loading projects…</p>
        ) : projects.data.length === 0 ? (
          <p className="rounded-lg border border-dashed bg-card px-5 py-7 text-center text-muted-foreground" data-empty>
            No projects yet. Create one with <b>New project</b>.
          </p>
        ) : (
          projects.data.map((project) => (
            <section key={project.id} className="grid gap-2" data-project={project.id} aria-labelledby={`project-${project.id}`}>
              <header className="mt-1 flex flex-wrap items-center gap-x-2 gap-y-1">
                <h2
                  id={`project-${project.id}`}
                  className="flex flex-wrap items-baseline gap-x-2.5 text-lead font-semibold wrap-anywhere"
                >
                  {project.name}{' '}
                  <span className="text-meta font-medium text-ink-3">
                    {plural(project.milestones.length, 'milestone')}
                  </span>
                </h2>
                <Button
                  type="button"
                  variant="ghost"
                  size="xs"
                  data-rename-project={project.id}
                  onClick={() => {
                    setForm({ kind: 'project', project })
                  }}
                >
                  Rename<span className="sr-only"> project {project.name}</span>
                </Button>
                <Button
                  type="button"
                  variant="outline"
                  size="xs"
                  className="ml-auto"
                  data-new-milestone={project.id}
                  onClick={() => {
                    setForm({ kind: 'milestone', project })
                  }}
                >
                  + New milestone<span className="sr-only"> in {project.name}</span>
                </Button>
              </header>
              <div className="overflow-hidden rounded-lg border bg-card">
                {project.milestones.length === 0 ? (
                  <p className="p-3 text-meta text-ink-3">No milestones yet.</p>
                ) : (
                  project.milestones.map((milestone) => (
                    <MilestoneRow
                      key={milestone.id}
                      milestone={milestone}
                      progress={milestoneProgress(issues.data.rows, workflow.data.statuses, milestone.id)}
                      prefix={workspace.issue_prefix}
                      open={open === milestone.id}
                      onToggle={() => {
                        setOpen((now) => (now === milestone.id ? null : milestone.id))
                      }}
                      onEdit={() => {
                        setForm({ kind: 'milestone', project, milestone })
                      }}
                    />
                  ))
                )}
              </div>
            </section>
          ))
        )}
      </main>

      <Dialog
        open={form !== null}
        onOpenChange={(next) => {
          if (!next) setForm(null)
        }}
      >
        {form && (
          <ProjectForm
            workspaceId={workspace.id}
            form={form}
            onClose={() => {
              setForm(null)
            }}
          />
        )}
      </Dialog>

      <ShortcutStrip
        title="Projects shortcuts"
        items={STRIPS.projects}
        actions={{
          ...screenActions,
          next: () => {
            focusNext(ITEMS)
          },
          // The browser's Enter on a button or a link is its click.
          open: () => focused(ITEMS)?.click(),
          issues: toList,
        }}
      />
    </div>
  )
}
