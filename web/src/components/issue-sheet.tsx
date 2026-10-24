// The side sheet Enter opens: the issue's fields, with title, assignee, labels, parent, project and milestone
// editable.
import type { ReactNode } from 'react'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Sheet, SheetContent, SheetDescription, SheetTitle } from '@/components/ui/sheet'
import { Assignee, Gates, LockBadge, PullRequest, Run, StatusDot, TransitionNote } from '@/components/atoms'
import { StatusMenu } from '@/components/status-menu'
import {
  agentName,
  age,
  rowView,
  type ActorInfo,
  type IssueRow,
  type RowView,
  type StatusInfo,
  type Transition,
} from '@/model/issues'
import { useIssues, useProjects, type Edit } from '@/lib/queries'

const UNASSIGNED = 'unassigned'
const NONE = 'none'

/** An input that saves on Enter or blur when its value changed. Keyed by the saved value, so a rollback shows. */
function SavedInput({
  value,
  label,
  onSave,
  allowEmpty = false,
  placeholder,
}: {
  value: string
  label: string
  onSave: (value: string) => void
  allowEmpty?: boolean
  placeholder?: string
}) {
  const save = (input: HTMLInputElement) => {
    const next = input.value.trim()
    if ((next || allowEmpty) && next !== value) onSave(next)
    else input.value = value
  }
  return (
    <Input
      key={value}
      defaultValue={value}
      aria-label={label}
      placeholder={placeholder}
      className="h-8"
      onBlur={(event) => {
        save(event.currentTarget)
      }}
      onKeyDown={(event) => {
        if (event.key === 'Enter') {
          event.preventDefault()
          save(event.currentTarget)
        }
        if (event.key === 'Escape') event.currentTarget.value = value
      }}
    />
  )
}

function Field({ label, children }: { label: string; children: ReactNode }) {
  return (
    <>
      <dt className="pt-1 text-meta text-ink-3">{label}</dt>
      <dd className="m-0 min-w-0">{children}</dd>
    </>
  )
}

export function IssueSheet({
  workspaceId,
  row,
  status,
  statuses,
  actors,
  transition,
  now,
  onClose,
  onEdit,
  onMove,
  onDismiss,
}: {
  workspaceId: string
  row: IssueRow | undefined
  status: StatusInfo | undefined
  statuses: readonly StatusInfo[]
  actors: readonly ActorInfo[]
  transition: Transition | undefined
  now: Date
  onClose: () => void
  onEdit: (edit: Edit) => void
  onMove: (status: StatusInfo) => void
  onDismiss: () => void
}) {
  const assignable = actors.filter((actor) => actor.role === 'person' || actor.role === 'agent')
  return (
    <Sheet
      open={row !== undefined}
      onOpenChange={(open) => {
        if (!open) onClose()
      }}
    >
      <SheetContent className="w-[min(480px,100vw)] gap-0 overflow-y-auto bg-card p-4.5 sm:max-w-none">
        {row && (
          <SheetBody
            row={row}
            view={rowView(row, now)}
            {...{ workspaceId, status, statuses, assignable, transition, now, onEdit, onMove, onDismiss }}
          />
        )}
      </SheetContent>
    </Sheet>
  )
}

function SheetBody({
  workspaceId,
  row,
  view,
  status,
  statuses,
  assignable,
  transition,
  now,
  onEdit,
  onMove,
  onDismiss,
}: {
  workspaceId: string
  row: IssueRow
  view: RowView
  status: StatusInfo | undefined
  statuses: readonly StatusInfo[]
  assignable: readonly ActorInfo[]
  transition: Transition | undefined
  now: Date
  onEdit: (edit: Edit) => void
  onMove: (status: StatusInfo) => void
  onDismiss: () => void
}) {
  const { issue } = row
  const projects = useProjects(workspaceId).data ?? []
  const parentTitle = useIssues(workspaceId).data?.rows.find((other) => other.issue.id === issue.parent)?.issue.title
  const milestones = projects.find((project) => project.id === issue.project?.id)?.milestones ?? []
  const passWhen = milestones.find((milestone) => milestone.id === issue.milestone?.id)?.pass_when
  return (
    <>
      <SheetDescription className="font-mono text-meta text-ink-3">{view.id}</SheetDescription>
      <SheetTitle className="mt-1.5 mb-3 pr-8 text-title">{view.title}</SheetTitle>
      <dl className="m-0 grid grid-cols-[110px_minmax(0,1fr)] gap-x-3 gap-y-2">
        <Field label="Title">
          <SavedInput
            value={view.title}
            label="Title"
            onSave={(title) => {
              onEdit({ id: view.id, title })
            }}
          />
        </Field>
        <Field label="Status">
          <div className="flex flex-wrap items-center gap-2">
            <StatusDot status={view.status} /> {view.statusName} {status && <LockBadge status={status} />}
            <StatusMenu issueId={view.id} current={view.status} statuses={statuses} onChoose={onMove}>
              <Button variant="outline" size="sm" className="h-6.5 px-2 text-meta">
                Change status <kbd>s</kbd>
              </Button>
            </StatusMenu>
          </div>
          {transition && (
            <div className="mt-1.5">
              <TransitionNote transition={transition} onDismiss={onDismiss} />
            </div>
          )}
        </Field>
        <Field label="Assignee">
          <Select
            value={row.issue.assignee?.id ?? UNASSIGNED}
            onValueChange={(id) => {
              onEdit({ id: view.id, assignee: assignable.find((actor) => actor.id === id) ?? null })
            }}
          >
            <SelectTrigger size="sm" className="w-full" aria-label="Assignee">
              <SelectValue>
                <Assignee view={view.assignee} />
              </SelectValue>
            </SelectTrigger>
            <SelectContent>
              <SelectItem value={UNASSIGNED}>Unassigned</SelectItem>
              {assignable.map((actor) => (
                <SelectItem key={actor.id} value={actor.id}>
                  {actor.role === 'agent' ? `${agentName(actor.kind)} · ${actor.name}` : actor.name}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </Field>
        <Field label="Labels">
          <SavedInput
            value={view.labels.join(', ')}
            label="Labels"
            allowEmpty
            onSave={(text) => {
              onEdit({
                id: view.id,
                labels: text
                  .split(',')
                  .map((label) => label.trim())
                  .filter(Boolean),
              })
            }}
          />
        </Field>
        <Field label="Parent">
          <SavedInput
            value={issue.parent ?? ''}
            label="Parent"
            placeholder="None"
            allowEmpty
            onSave={(parent) => {
              onEdit({ id: view.id, parent: parent || null })
            }}
          />
          {parentTitle && <div className="truncate text-meta text-ink-3">{parentTitle}</div>}
        </Field>
        <Field label="Project">
          <Select
            value={issue.project?.id ?? NONE}
            onValueChange={(id) => {
              const chosen = projects.find((project) => project.id === id)
              // A milestone is in its project, so another project leaves none.
              onEdit({ id: view.id, project: chosen ? { id: chosen.id, name: chosen.name } : null, milestone: null })
            }}
          >
            <SelectTrigger size="sm" className="w-full" aria-label="Project">
              <SelectValue>{issue.project?.name ?? 'No project'}</SelectValue>
            </SelectTrigger>
            <SelectContent>
              <SelectItem value={NONE}>No project</SelectItem>
              {projects.map((project) => (
                <SelectItem key={project.id} value={project.id}>
                  {project.name}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </Field>
        <Field label="Milestone">
          <Select
            value={issue.milestone?.id ?? NONE}
            disabled={!issue.project}
            onValueChange={(id) => {
              const chosen = milestones.find((milestone) => milestone.id === id)
              onEdit({ id: view.id, milestone: chosen ? { id: chosen.id, name: chosen.name } : null })
            }}
          >
            <SelectTrigger size="sm" className="w-full" aria-label="Milestone">
              <SelectValue>{issue.milestone?.name ?? 'No milestone'}</SelectValue>
            </SelectTrigger>
            <SelectContent>
              <SelectItem value={NONE}>No milestone</SelectItem>
              {milestones.map((milestone) => (
                <SelectItem key={milestone.id} value={milestone.id}>
                  {milestone.name}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
          {passWhen && <div className="text-meta text-ink-3">PASS when {passWhen}</div>}
        </Field>
        <Field label="Run">
          <Run view={view.run} detail />
        </Field>
        <Field label="Pull request">
          <PullRequest view={view.pullRequest} link />
        </Field>
        <Field label="Gates">
          <Gates view={view.gates} />
          {row.gates && <div className="text-meta text-ink-3">{view.gates.title}</div>}
        </Field>
        <Field label="Updated">{age(row.issue.updated_at, now)} ago</Field>
      </dl>
      {row.issue.body && <div className="mt-4.5 border-t pt-3 whitespace-pre-wrap wrap-anywhere">{row.issue.body}</div>}
    </>
  )
}
