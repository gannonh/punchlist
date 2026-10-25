// The list's menu for a selection: a right-click opens it with a picker per property in a submenu, and a key
// opens one picker at the focused row. Both pick through the same `Picker`.
import { useEffect, useRef, useState, type KeyboardEvent, type ReactNode } from 'react'
import { toast } from 'sonner'
import {
  ContextMenu,
  ContextMenuCheckboxItem,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuLabel,
  ContextMenuSeparator,
  ContextMenuShortcut,
  ContextMenuSub,
  ContextMenuSubContent,
  ContextMenuSubTrigger,
  ContextMenuTrigger,
} from '@/components/ui/context-menu'
import {
  DropdownMenu,
  DropdownMenuCheckboxItem,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { agentName, type ActorInfo, type IssueRow } from '@/model/issues'
import { labelMark, mark, withLabel, type Mark } from '@/model/selection'
import type { Edit } from '@/lib/queries'
import type { components } from '@/api/schema'

type Project = components['schemas']['Project']

export type PickerKind = 'assignee' | 'labels' | 'project' | 'milestone'

/** Each picker with the key that opens it on the list: `key` is `KeyboardEvent.key`, `hint` how the menu shows it. */
export const PICKERS: readonly { kind: PickerKind; name: string; key: string; hint: string }[] = [
  { kind: 'assignee', name: 'Assignee', key: 'a', hint: 'A' },
  { kind: 'labels', name: 'Labels', key: 'l', hint: 'L' },
  { kind: 'project', name: 'Project', key: 'P', hint: '⇧P' },
  { kind: 'milestone', name: 'Milestone', key: 'M', hint: '⇧M' },
]

/** What a picker can choose from: the workspace's actors, projects and the labels its issues have. */
export interface PickerContext {
  actors: readonly ActorInfo[]
  projects: readonly Project[]
  labels: readonly string[]
}

/** A choice made: the toast's words, one edit per issue it changes, and how many already had it. */
export type Apply = (action: string, edits: readonly Edit[], untouched: number) => void

interface Option {
  value: string
  text: string
  mark: Mark
}

interface Model {
  name: string
  /** Labels: each choice adds or removes one, and the menu stays open. */
  multi: boolean
  options: Option[]
  plan: (value: string) => { action: string; edits: Edit[] }
}

const NONE = 'none'

function model(kind: PickerKind, targets: readonly IssueRow[], context: PickerContext): Model {
  const issues = targets.map((row) => row.issue)
  const markOf = (has: (issue: IssueRow['issue']) => boolean) => mark(issues.map(has))
  switch (kind) {
    case 'assignee': {
      const assignable = context.actors.filter((actor) => actor.role === 'person' || actor.role === 'agent')
      const text = (actor: ActorInfo) => (actor.role === 'agent' ? `${agentName(actor.kind)} · ${actor.name}` : actor.name)
      return {
        name: 'Assignee',
        multi: false,
        options: [
          { value: NONE, text: 'Unassigned', mark: markOf((issue) => !issue.assignee) },
          ...assignable.map((actor) => ({
            value: actor.id,
            text: text(actor),
            mark: markOf((issue) => issue.assignee?.id === actor.id),
          })),
        ],
        plan: (value) => {
          const actor = assignable.find((a) => a.id === value) ?? null
          return {
            action: actor ? `Set Assignee to ${text(actor)}` : 'Cleared Assignee',
            edits: issues
              .filter((issue) => (issue.assignee?.id ?? null) !== (actor?.id ?? null))
              .map((issue) => ({ id: issue.id, assignee: actor })),
          }
        },
      }
    }
    case 'labels':
      return {
        name: 'Labels',
        multi: true,
        options: context.labels.map((label) => ({
          value: label,
          text: label,
          mark: labelMark(
            issues.map((issue) => issue.labels ?? []),
            label,
          ),
        })),
        plan: (label) => {
          const add = labelMark(
            issues.map((issue) => issue.labels ?? []),
            label,
          ) !== true
          return {
            action: add ? `Added label ${label}` : `Removed label ${label}`,
            edits: issues
              .filter((issue) => (issue.labels ?? []).includes(label) !== add)
              .map((issue) => ({ id: issue.id, labels: withLabel(issue.labels ?? [], label, add) })),
          }
        },
      }
    case 'project':
      return {
        name: 'Project',
        multi: false,
        options: [
          { value: NONE, text: 'No project', mark: markOf((issue) => !issue.project) },
          ...context.projects.map((project) => ({
            value: project.id,
            text: project.name,
            mark: markOf((issue) => issue.project?.id === project.id),
          })),
        ],
        plan: (value) => {
          const project = context.projects.find((p) => p.id === value)
          const ref = project ? { id: project.id, name: project.name } : null
          return {
            action: project ? `Set Project to ${project.name}` : 'Cleared Project',
            // A milestone is in its project, so another project leaves none.
            edits: issues
              .filter((issue) => (issue.project?.id ?? null) !== (ref?.id ?? null))
              .map((issue) => ({ id: issue.id, project: ref, milestone: null })),
          }
        },
      }
    case 'milestone': {
      // The selection's one project's milestones, else every project's, named with their project.
      const shared = new Set(issues.map((issue) => issue.project?.id ?? NONE))
      const only = shared.size === 1 ? context.projects.find((p) => shared.has(p.id)) : undefined
      const milestones = (only ? [only] : context.projects).flatMap((project) =>
        project.milestones.map((milestone) => ({ project, milestone })),
      )
      return {
        name: 'Milestone',
        multi: false,
        options: [
          { value: NONE, text: 'No milestone', mark: markOf((issue) => !issue.milestone) },
          ...milestones.map(({ project, milestone }) => ({
            value: milestone.id,
            text: only ? milestone.name : `${project.name} · ${milestone.name}`,
            mark: markOf((issue) => issue.milestone?.id === milestone.id),
          })),
        ],
        plan: (value) => {
          const chosen = milestones.find(({ milestone }) => milestone.id === value)
          const ref = chosen ? { id: chosen.milestone.id, name: chosen.milestone.name } : null
          const project = chosen ? { id: chosen.project.id, name: chosen.project.name } : null
          return {
            action: chosen ? `Set Milestone to ${chosen.milestone.name}` : 'Cleared Milestone',
            // A milestone from another project moves the issue to that project too.
            edits: issues
              .filter((issue) => (issue.milestone?.id ?? null) !== (ref?.id ?? null))
              .map((issue) => ({
                id: issue.id,
                milestone: ref,
                ...(project && issue.project?.id !== project.id && { project }),
              })),
          }
        },
      }
    }
  }
}

/** The menu parts a picker renders with: a context menu's or a dropdown menu's. */
interface Parts {
  Item: (props: { onSelect: (event: Event) => void; children: ReactNode }) => ReactNode
  CheckboxItem: (props: {
    checked: Mark
    onSelect: (event: Event) => void
    className?: string
    'data-option'?: string
    children: ReactNode
  }) => ReactNode
}

const CONTEXT_PARTS: Parts = { Item: ContextMenuItem, CheckboxItem: ContextMenuCheckboxItem }
const DROPDOWN_PARTS: Parts = { Item: DropdownMenuItem, CheckboxItem: DropdownMenuCheckboxItem }

const ITEMS = '[role^="menuitem"]'

/**
 * Tab and Shift+Tab move through a menu's filter and items as the arrows do; Radix menus otherwise keep Tab
 * from moving focus at all. A submenu's Tab reaches its menu's handler too, through the portal, already handled.
 */
function tabThrough(event: KeyboardEvent<HTMLElement>) {
  if (event.key !== 'Tab' || event.defaultPrevented) return
  event.preventDefault()
  const stops = [...event.currentTarget.querySelectorAll<HTMLElement>(`input, ${ITEMS}`)]
  const at = stops.indexOf(document.activeElement as HTMLElement)
  stops.at((at + (event.shiftKey ? -1 : 1)) % stops.length)?.focus()
}

function Picker({
  kind,
  targets,
  context,
  parts,
  onApply,
}: {
  kind: PickerKind
  targets: readonly IssueRow[]
  context: PickerContext
  parts: Parts
  onApply: Apply
}) {
  const [text, setText] = useState('')
  const input = useRef<HTMLInputElement>(null)
  // After Radix has focused the menu itself.
  useEffect(() => {
    const frame = requestAnimationFrame(() => input.current?.focus())
    return () => {
      cancelAnimationFrame(frame)
    }
  }, [])
  const picker = model(kind, targets, context)
  const q = text.trim()
  const shown = picker.options.filter((option) => option.text.toLowerCase().includes(q.toLowerCase()))
  const fresh = picker.multi && q !== '' && !picker.options.some((option) => option.value === q)
  const choose = (value: string) => {
    const { action, edits } = picker.plan(value)
    onApply(action, edits, targets.length - edits.length)
  }
  return (
    <>
      <input
        ref={input}
        value={text}
        placeholder={`${picker.name}…`}
        aria-label={`Filter ${picker.name.toLowerCase()}`}
        className="mb-1 h-7.5 w-full rounded-md border bg-card px-2 focus:outline-2 focus:outline-offset-1 focus:outline-ring"
        onChange={(event) => {
          setText(event.target.value)
        }}
        // The menu would take letters for its typeahead and the arrows for its items; the filter keeps them,
        // except Down, Enter, Tab and Esc.
        onKeyDown={(event) => {
          const first = event.currentTarget.parentElement?.querySelector<HTMLElement>(ITEMS)
          if (event.key === 'ArrowDown') {
            event.preventDefault()
            first?.focus()
          } else if (event.key === 'Enter') {
            event.preventDefault()
            first?.click()
          } else if (event.key !== 'Escape' && event.key !== 'Tab') event.stopPropagation()
        }}
      />
      {shown.map((option) => (
        <parts.CheckboxItem
          key={option.value}
          className="text-body"
          checked={option.mark}
          data-option={option.text}
          onSelect={(event) => {
            if (picker.multi) event.preventDefault()
            choose(option.value)
          }}
        >
          <span className="truncate">{option.text}</span>
        </parts.CheckboxItem>
      ))}
      {fresh && (
        <parts.Item
          onSelect={(event) => {
            event.preventDefault()
            choose(q)
            setText('')
          }}
        >
          Add label “{q}”
        </parts.Item>
      )}
      {shown.length === 0 && !fresh && <p className="px-2 py-1.5 text-meta text-ink-3">No matches</p>}
    </>
  )
}

const targetName = (targets: readonly IssueRow[]) =>
  targets.length === 1 ? (targets[0]?.issue.id ?? '') : `${String(targets.length)} issues`

/**
 * The right-click menu over the list. `onTarget` gets the event's row, or null off a row, and answers the issues
 * the menu acts on; none leaves the browser's own menu shut and opens nothing.
 */
export function RowMenu({
  targets,
  context,
  onTarget,
  onOpenChange,
  onApply,
  children,
}: {
  targets: readonly IssueRow[]
  context: PickerContext
  onTarget: (id: string | null) => boolean
  onOpenChange: (open: boolean) => void
  onApply: Apply
  children: ReactNode
}) {
  return (
    <ContextMenu onOpenChange={onOpenChange}>
      <ContextMenuTrigger
        asChild
        onContextMenu={(event) => {
          const id = (event.target as Element).closest('[data-issue]')?.getAttribute('data-issue') ?? null
          if (!onTarget(id)) event.preventDefault()
        }}
      >
        <div>{children}</div>
      </ContextMenuTrigger>
      <ContextMenuContent
        className="min-w-52"
        aria-label={`Change ${targetName(targets)}`}
        data-row-menu
        onKeyDown={tabThrough}
      >
        <ContextMenuLabel className="text-caption font-normal text-ink-3">{targetName(targets)}</ContextMenuLabel>
        {PICKERS.map(({ kind, name, hint }) => (
          <ContextMenuSub key={kind}>
            <ContextMenuSubTrigger className="text-body [&>svg:last-child]:ml-1">
              {name}
              <ContextMenuShortcut>{hint}</ContextMenuShortcut>
            </ContextMenuSubTrigger>
            <ContextMenuSubContent
              className="max-h-80 w-64 overflow-y-auto"
              aria-label={name}
              collisionPadding={12}
              onKeyDown={tabThrough}
            >
              <Picker kind={kind} targets={targets} context={context} parts={CONTEXT_PARTS} onApply={onApply} />
            </ContextMenuSubContent>
          </ContextMenuSub>
        ))}
        <ContextMenuSeparator />
        <ContextMenuItem
          className="text-body"
          onSelect={() => {
            const ids = targets.map((row) => row.issue.id).join(', ')
            navigator.clipboard.writeText(ids).then(
              () => toast.success(`Copied ${ids}`),
              () => toast.error('The browser did not allow copying.'),
            )
          }}
        >
          Copy ID
        </ContextMenuItem>
      </ContextMenuContent>
    </ContextMenu>
  )
}

/** One picker opened by its key, at the point `at` (the focused row). */
export function PickerMenu({
  kind,
  at,
  targets,
  context,
  onClose,
  onApply,
}: {
  kind: PickerKind
  at: { left: number; top: number }
  targets: readonly IssueRow[]
  context: PickerContext
  onClose: () => void
  onApply: Apply
}) {
  const name = PICKERS.find((picker) => picker.kind === kind)?.name ?? kind
  return (
    <DropdownMenu
      open
      onOpenChange={(open) => {
        if (!open) onClose()
      }}
    >
      <DropdownMenuTrigger asChild>
        <span className="pointer-events-none fixed size-0" style={at} aria-hidden="true" />
      </DropdownMenuTrigger>
      <DropdownMenuContent
        align="start"
        className="max-h-80 w-64"
        aria-label={`${name} of ${targetName(targets)}`}
        collisionPadding={12}
        onKeyDown={tabThrough}
      >
        <Picker kind={kind} targets={targets} context={context} parts={DROPDOWN_PARTS} onApply={onApply} />
      </DropdownMenuContent>
    </DropdownMenu>
  )
}
