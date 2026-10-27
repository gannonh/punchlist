import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, useSyncExternalStore } from 'react'
import { useNavigate } from '@tanstack/react-router'
import { ChevronDown, Plus } from 'lucide-react'
import { Button } from '@/components/ui/button'
import {
  DropdownMenu,
  DropdownMenuCheckboxItem,
  DropdownMenuContent,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { AccountMenu } from '@/components/account-menu'
import { AppHeader, inField } from '@/components/app-header'
import type { Workspace } from '@/model/workspaces'
import { BoardView, ListView } from '@/components/issue-views'
import { IssueSheet } from '@/components/issue-sheet'
import { NewIssue } from '@/components/new-issue'
import { StatusMenu } from '@/components/status-menu'
import { PICKERS, PickerMenu, RowMenu, type PickerContext, type PickerKind } from '@/components/bulk-menu'
import { addRange, keepShown, toggle, type Selection } from '@/model/selection'
import {
  AGENT_NAMES,
  distinctLabels,
  filterRows,
  groupByStatus,
  neighbourStatus,
  type Filter,
  type Group,
  type StatusInfo,
  type Who,
} from '@/model/issues'
import {
  useActors,
  useBulkEdit,
  useEditOne,
  useEventStream,
  useInbox,
  useIssues,
  useProjects,
  usePullRequests,
  useSearch,
  useTransitions,
  useWorkflow,
} from '@/lib/queries'

export type Layout = 'list' | 'board'

/**
 * What the list's URL holds: the layout, the project and milestone filters, each `'none'` or an id, and the
 * chosen labels and status keys.
 */
export interface ListSearch {
  layout?: 'board'
  project?: string
  milestone?: string
  label?: string[]
  status?: string[]
}

/**
 * A menu of checkboxes whose trigger reads `<name>` or `<name>: <chosen, in option order>`. A chosen value
 * no option has, from a link or a label no issue has any more, is listed too, so it can be unchecked.
 */
function CheckMenu({
  name,
  label,
  options,
  chosen,
  onChange,
}: {
  name: string
  label: string
  options: readonly { value: string; text: string }[]
  chosen: readonly string[]
  onChange: (chosen: string[]) => void
}) {
  const all = [
    ...options,
    ...chosen.filter((value) => !options.some((o) => o.value === value)).map((value) => ({ value, text: value })),
  ]
  const shown = all.filter((option) => chosen.includes(option.value))
  const text = shown.length > 0 ? `${name}: ${shown.map((option) => option.text).join(', ')}` : name
  return (
    <DropdownMenu>
      <DropdownMenuTrigger
        aria-label={label}
        title={text}
        className="inline-flex h-7.5 max-w-55 cursor-pointer items-center gap-1 rounded-md border bg-card px-2 focus:outline-2 focus:outline-offset-1 focus:outline-ring"
      >
        <span className="truncate">{text}</span>
        <ChevronDown className="size-3.5 shrink-0 text-muted-foreground" aria-hidden="true" />
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" aria-label={label} collisionPadding={12}>
        {all.length === 0 && <p className="px-2 py-1.5 text-meta text-ink-3">No {name.toLowerCase()}</p>}
        {all.map((option) => (
          <DropdownMenuCheckboxItem
            key={option.value}
            className="text-body"
            checked={chosen.includes(option.value)}
            onSelect={(event) => {
              event.preventDefault()
            }}
            onCheckedChange={(checked) => {
              onChange(
                all
                  .map((o) => o.value)
                  .filter((value) => (value === option.value ? checked : chosen.includes(value))),
              )
            }}
          >
            {option.text}
          </DropdownMenuCheckboxItem>
        ))}
      </DropdownMenuContent>
    </DropdownMenu>
  )
}

const NARROW = '(max-width: 767px)'
const subscribeNarrow = (notify: () => void) => {
  const query = window.matchMedia(NARROW)
  query.addEventListener('change', notify)
  return () => {
    query.removeEventListener('change', notify)
  }
}
const useNarrow = () => useSyncExternalStore(subscribeNarrow, () => window.matchMedia(NARROW).matches)

/** The order j and k walk: every visible issue in group order, or on a narrow board the shown column's. */
function navOrder(groups: Group[], layout: Layout, narrow: boolean, column: string): string[] {
  const shown = layout === 'board' && narrow ? groups.filter((g) => g.status.key === column) : groups
  return shown.flatMap((g) => g.rows.map((row) => row.issue.id))
}

const SHORTCUTS: Record<Layout, [string, string][]> = {
  list: [
    ['j', 'next issue'],
    ['k', 'previous issue'],
    ['Enter', 'open issue'],
    ['s', 'change status'],
    ['x', 'select'],
    ['⇧j ⇧k', 'select range'],
    ['a l ⇧p ⇧m', 'assignee, labels, project, milestone'],
    ['/', 'filter'],
    ['1 2', 'switch layout'],
    ['g p', 'pull requests'],
    ['g m', 'projects'],
    ['Esc', 'close'],
  ],
  board: [
    ['j', 'next issue'],
    ['k', 'previous issue'],
    ['Enter', 'open issue'],
    ['s', 'change status'],
    ['h', 'move issue left'],
    ['l', 'move issue right'],
    ['/', 'filter'],
    ['1 2', 'switch layout'],
    ['g p', 'pull requests'],
    ['g m', 'projects'],
    ['Esc', 'close'],
  ],
}

export function useNow(): Date {
  const [now, setNow] = useState(() => new Date())
  useEffect(() => {
    const timer = setInterval(() => {
      setNow(new Date())
    }, 30_000)
    return () => {
      clearInterval(timer)
    }
  }, [])
  return now
}

export function IssuesScreen({
  workspace,
  me,
  search: url,
  onSearch,
}: {
  workspace: Workspace
  me: string
  search: ListSearch
  onSearch: (search: ListSearch) => void
}) {
  const workspaceId = workspace.id
  const issues = useIssues(workspaceId)
  const workflow = useWorkflow(workspaceId)
  const actors = useActors(workspaceId)
  const projects = useProjects(workspaceId)
  const edit = useEditOne(workspaceId)
  const bulkEdit = useBulkEdit(workspaceId)
  const navigate = useNavigate()
  const { transitions, request, dismiss } = useTransitions(workspaceId)
  useEventStream(workspaceId, issues.isSuccess)
  const pullRequests = usePullRequests(workspaceId, issues.isSuccess)
  const inbox = useInbox(workspaceId, issues.isSuccess)

  const [local, setLocal] = useState<Pick<Filter, 'text' | 'who'>>({ text: '', who: '' })
  const layout: Layout = url.layout ?? 'list'
  const filter = useMemo<Filter>(
    () => ({
      ...local,
      project: url.project ?? '',
      milestone: url.milestone ?? '',
      labels: url.label ?? [],
      statuses: url.status ?? [],
    }),
    [local, url.project, url.milestone, url.label, url.status],
  )
  // The list's checked rows; `selected` is the row the keys act on.
  const [checked, setChecked] = useState<Selection>(() => new Set())
  const anchor = useRef<string | null>(null)
  const onLayout = useCallback(
    (next: Layout) => {
      onSearch({ ...url, layout: next === 'board' ? 'board' : undefined })
      setChecked(new Set())
    },
    [onSearch, url],
  )
  const [selected, setSelected] = useState<string | null>(null)
  const [column, setColumn] = useState<string | null>(null)
  const [sheet, setSheet] = useState<string | null>(null)
  const [menu, setMenu] = useState<{ id: string; left: number; top: number } | null>(null)
  const [picker, setPicker] = useState<{ kind: PickerKind; ids: string[]; left: number; top: number } | null>(null)
  const [rowMenu, setRowMenu] = useState<{ ids: string[]; open: boolean }>({ ids: [], open: false })
  const [creating, setCreating] = useState(false)
  const [hideClosed, setHideClosed] = useState(true)
  const search = useRef<HTMLInputElement>(null)
  const narrow = useNarrow()

  const statuses = useMemo(() => workflow.data?.statuses ?? [], [workflow.data])
  const rows = useMemo(() => issues.data?.rows ?? [], [issues.data])
  const closed = useMemo(() => statuses.filter((status) => status.closed), [statuses])
  // Chosen statuses replace `hide Done and Canceled`, so choosing Done shows Done issues.
  const hiding = hideClosed && filter.statuses.length === 0
  const searched = useSearch(workspaceId, local.text)
  const hitIds = useMemo(() => searched?.hits.map((hit) => hit.issue_id) ?? [], [searched])
  const snippets = useMemo(
    () => new Map(searched?.hits.flatMap((hit) => (hit.snippet ? [[hit.issue_id, hit.snippet] as const] : []))),
    [searched],
  )
  const visible = useMemo(() => {
    const shown = filterRows(rows, filter, hitIds)
    return hiding ? shown.filter((row) => !closed.some((status) => status.key === row.issue.status)) : shown
  }, [rows, filter, hitIds, hiding, closed])
  const columns = useMemo(
    () => (filter.statuses.length > 0 ? statuses.filter((status) => filter.statuses.includes(status.key)) : statuses),
    [statuses, filter.statuses],
  )
  const groups = useMemo(() => groupByStatus(visible, columns), [visible, columns])
  // A row a filter hides leaves the selection.
  const shownChecked = keepShown(
    checked,
    visible.map((row) => row.issue.id),
  )
  if (shownChecked !== checked) setChecked(shownChecked)
  const labelOptions = useMemo(
    () => distinctLabels(rows).map((label) => ({ value: label, text: label })),
    [rows],
  )
  const statusOptions = useMemo(() => statuses.map((status) => ({ value: status.key, text: status.name })), [statuses])
  const shownColumn = column ?? groups.find((g) => g.rows.length > 0)?.status.key ?? statuses[0]?.key ?? ''
  const order = useMemo(() => navOrder(groups, layout, narrow, shownColumn), [groups, layout, narrow, shownColumn])
  const current = selected !== null && order.includes(selected) ? selected : (order[0] ?? null)
  const rowById = useCallback((id: string | null) => rows.find((row) => row.issue.id === id), [rows])
  const statusByKey = useCallback((key: string | undefined) => statuses.find((s) => s.key === key), [statuses])
  const now = useNow()
  const currentRef = useRef(current)
  const orderRef = useRef(order)
  useLayoutEffect(() => {
    currentRef.current = current
    orderRef.current = order
  })

  useEffect(() => {
    if (current) document.querySelector(`[data-issue="${current}"]`)?.scrollIntoView({ block: 'nearest' })
  }, [current])

  const move = useCallback(
    async (id: string, to: StatusInfo) => {
      const moved = await request(id, to)
      if (moved && layout === 'board' && narrow) setColumn(to.key)
    },
    [request, layout, narrow],
  )

  // Reads the selection through a ref, so a new selection does not re-render every row.
  const pick = useCallback((id: string) => {
    if (id === currentRef.current) setSheet(id)
    else setSelected(id)
  }, [])

  const below = (id: string) => {
    const rect = document.querySelector(`[data-issue="${id}"]`)?.getBoundingClientRect()
    return { left: (rect?.left ?? 20) + 12, top: rect?.bottom ?? 80 }
  }

  const openMenu = useCallback((id: string) => {
    setMenu({ id, ...below(id) })
  }, [])

  // A click on a row's box toggles it; with Shift, or a Shift-click on the row, it adds the rows from the last one
  // toggled (else the focused row) to it.
  const check = useCallback((id: string, range: boolean) => {
    const from = anchor.current ?? currentRef.current
    setChecked((selection) => (range && from ? addRange(selection, orderRef.current, from, id) : toggle(selection, id)))
    anchor.current = id
  }, [])

  // What the menu and the picker keys act on: the checked rows in list order, else the focused row.
  const targets = useCallback(
    () => (checked.size > 0 ? order.filter((id) => checked.has(id)) : current ? [current] : []),
    [checked, order, current],
  )

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.metaKey || event.ctrlKey || event.altKey) return
      if (inField(event.target)) {
        if (event.key === 'Escape') (event.target as HTMLElement).blur()
        return
      }
      // A menu's Escape closes it in the capture phase, and the re-render can add this listener before the event
      // bubbles here; the menu marks it handled.
      if (event.defaultPrevented) return
      if (sheet || menu || creating || picker || rowMenu.open) return
      const step = (delta: number) => {
        if (order.length === 0) return
        const index = Math.max(0, current ? order.indexOf(current) : 0)
        const next = order[Math.min(order.length - 1, Math.max(0, index + delta))] ?? null
        // Shift extends the selection from the focused row to the next.
        if (event.shiftKey && layout === 'list' && current && next) {
          setChecked((selection) => addRange(selection, order, current, next))
          anchor.current = next
        }
        setSelected(next)
      }
      const shift = (delta: number) => {
        const row = rowById(current)
        const to = row && neighbourStatus(statuses, row.issue.status, delta)
        if (row && to && transitions[row.issue.id]?.state !== 'pending') void move(row.issue.id, to)
      }
      const keys: Record<string, (() => void) | undefined> = {
        j: () => {
          step(1)
        },
        ArrowDown: () => {
          step(1)
        },
        k: () => {
          step(-1)
        },
        ArrowUp: () => {
          step(-1)
        },
        Enter: () => {
          if (!current) return
          const seq = snippets.get(current)?.comment_seq
          void navigate({ to: '/issues/$id', params: { id: current }, hash: seq == null ? undefined : `seq-${String(seq)}` })
        },
        s: () => {
          if (current) openMenu(current)
        },
        '/': () => search.current?.focus(),
        '1': () => {
          onLayout('list')
        },
        '2': () => {
          onLayout('board')
        },
        ...(layout === 'board' && {
          h: () => {
            shift(-1)
          },
          l: () => {
            shift(1)
          },
        }),
        ...(layout === 'list' && {
          x: () => {
            if (!current) return
            setChecked((selection) => toggle(selection, current))
            anchor.current = current
          },
          J: () => {
            step(1)
          },
          K: () => {
            step(-1)
          },
          Escape: () => {
            setChecked(new Set())
          },
          ...Object.fromEntries(
            PICKERS.map(({ kind, key }) => [
              key,
              () => {
                const ids = targets()
                const at = current ?? ids[0]
                if (at) setPicker({ kind, ids, ...below(at) })
              },
            ]),
          ),
        }),
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
  }, [sheet, menu, creating, picker, rowMenu.open, order, current, layout, statuses, transitions, snippets, rowById, move, openMenu, onLayout, navigate, targets])

  const filtered = Object.values({ ...filter }).some((value) => value.length > 0)
  const projectList = projects.data ?? []
  const allMilestones = projectList.flatMap((project) =>
    project.milestones.map((m) => ({ id: m.id, project: project.id, label: `${project.name} · ${m.name}` })),
  )
  const milestones = allMilestones.filter((m) => !filter.project || m.project === filter.project)
  const ref = (value: string) => value !== '' && value !== 'none'
  // An id in the URL that the select does not list: it shows that id rather than falling back to `Any`.
  const unknown = projects.data ? null : 'Loading…'
  const strayProject = ref(filter.project) && !projectList.some((p) => p.id === filter.project)
  const strayMilestone = ref(filter.milestone) && !milestones.some((m) => m.id === filter.milestone)
  const menuRow = rowById(menu?.id ?? null)
  const sheetRow = rowById(sheet)
  const rowsOf = (ids: readonly string[]) => ids.flatMap((id) => rowById(id) ?? [])
  const pickerContext: PickerContext = {
    actors: actors.data ?? [],
    projects: projectList,
    labels: labelOptions.map((option) => option.value),
  }
  const viewProps = { groups, selected: current, transitions, snippets, now, onPick: pick, onDismiss: dismiss }

  return (
    <div className="flex min-h-screen flex-col" data-last-seq={issues.data?.lastSeq}>
      <AppHeader workspace={workspace} current="issues" pullRequests={pullRequests.data?.length} inbox={inbox.data?.unread}>
        <div className="inline-flex rounded-lg border bg-card p-0.5" role="group" aria-label="Layout">
          {(['list', 'board'] as const).map((key, index) => (
            <button
              key={key}
              type="button"
              aria-pressed={layout === key}
              className="inline-flex cursor-pointer items-center gap-1.5 rounded-md px-2.5 py-1.25 text-muted-foreground aria-pressed:bg-foreground aria-pressed:text-card"
              onClick={() => {
                onLayout(key)
              }}
            >
              <kbd className="hidden opacity-75 in-aria-pressed:border-white/35 in-aria-pressed:bg-transparent in-aria-pressed:text-card sm:inline-block">{index + 1}</kbd>
              {key === 'list' ? 'List' : 'Board'}
            </button>
          ))}
        </div>
        <div className="flex w-full flex-wrap items-center gap-2 sm:ml-auto sm:w-auto">
          <input
            ref={search}
            type="search"
            placeholder="Filter issues  /"
            aria-label="Filter issues"
            value={local.text}
            className="h-7.5 flex-[1_1_120px] rounded-md border bg-card px-2 py-1.25 focus:outline-2 focus:outline-offset-1 focus:outline-ring sm:w-45 sm:flex-none"
            onChange={(event) => {
              setLocal((f) => ({ ...f, text: event.target.value }))
            }}
            // Esc leaves the filter (the page's key handler blurs it) with its text, which a search input would clear.
            onKeyDown={(event) => {
              if (event.key === 'Escape') event.preventDefault()
            }}
          />
          <select
            aria-label="Assignee filter"
            value={filter.who}
            className="h-7.5 rounded-md border bg-card px-2 focus:outline-2 focus:outline-offset-1 focus:outline-ring"
            onChange={(event) => {
              setLocal((f) => ({ ...f, who: event.target.value as Who }))
            }}
          >
            <option value="">Everyone</option>
            <option value="people">People</option>
            <option value="agents">Agents</option>
            {Object.entries(AGENT_NAMES).map(([kind, name]) => (
              <option key={kind} value={`agent:${kind}`}>
                {name}
              </option>
            ))}
            <option value="none">Unassigned</option>
          </select>
          <select
            aria-label="Project filter"
            value={filter.project}
            className="h-7.5 max-w-45 rounded-md border bg-card px-2 focus:outline-2 focus:outline-offset-1 focus:outline-ring"
            onChange={(event) => {
              const project = event.target.value
              const keep =
                project === '' ||
                !ref(filter.milestone) ||
                allMilestones.some((m) => m.id === filter.milestone && m.project === project)
              onSearch({ ...url, project: project || undefined, milestone: keep ? url.milestone : undefined })
            }}
          >
            <option value="">Any project</option>
            <option value="none">No project</option>
            {projectList.map((project) => (
              <option key={project.id} value={project.id}>
                {project.name}
              </option>
            ))}
            {strayProject && <option value={filter.project}>{unknown ?? 'Unknown project'}</option>}
          </select>
          <select
            aria-label="Milestone filter"
            value={filter.milestone}
            className="h-7.5 max-w-55 rounded-md border bg-card px-2 focus:outline-2 focus:outline-offset-1 focus:outline-ring"
            onChange={(event) => {
              onSearch({ ...url, milestone: event.target.value || undefined })
            }}
          >
            <option value="">Any milestone</option>
            <option value="none">No milestone</option>
            {milestones.map((milestone) => (
              <option key={milestone.id} value={milestone.id}>
                {milestone.label}
              </option>
            ))}
            {strayMilestone && (
              <option value={filter.milestone}>
                {unknown ?? allMilestones.find((m) => m.id === filter.milestone)?.label ?? 'Unknown milestone'}
              </option>
            )}
          </select>
          <CheckMenu
            name="Labels"
            label="Label filter"
            options={labelOptions}
            chosen={filter.labels}
            onChange={(label) => {
              onSearch({ ...url, label: label.length > 0 ? label : undefined })
            }}
          />
          <CheckMenu
            name="Statuses"
            label="Status filter"
            options={statusOptions}
            chosen={filter.statuses}
            onChange={(status) => {
              onSearch({ ...url, status: status.length > 0 ? status : undefined })
            }}
          />
          {closed.length > 0 && (
            <label className="flex items-center gap-1.5 text-meta text-ink-3">
              <input
                type="checkbox"
                checked={hideClosed}
                onChange={(event) => {
                  setHideClosed(event.target.checked)
                }}
              />
              hide {closed.map((status) => status.name).join(' and ')}
            </label>
          )}
          <span className="text-meta text-ink-3" data-count>
            {visible.length} of {rows.length}
          </span>
          {checked.size > 0 && (
            <span className="inline-flex items-center gap-1.5 text-meta font-medium" data-selection>
              {checked.size} selected
              <Button
                variant="outline"
                size="sm"
                className="h-6.5 px-2 text-meta"
                onClick={() => {
                  setChecked(new Set())
                }}
              >
                Clear
              </Button>
            </span>
          )}
          <Button
            size="sm"
            className="h-7.5"
            onClick={() => {
              setCreating(true)
            }}
          >
            <Plus /> New issue
          </Button>
          <AccountMenu me={me} />
        </div>
      </AppHeader>

      <main className="flex-1 px-3 py-2 sm:px-4 sm:py-3" aria-live="polite">
        {issues.isPending || workflow.isPending ? (
          <p className="p-6 text-center text-ink-3">Loading issues…</p>
        ) : issues.isError || workflow.isError ? (
          <p className="p-6 text-center text-bad" role="alert">
            Could not load the issues: {String(issues.error ?? workflow.error)}
          </p>
        ) : visible.length === 0 ? (
          <section
            className="mx-auto my-12 max-w-105 rounded-lg border border-dashed bg-card px-5 py-7 text-center"
            data-empty
          >
            <h2 className="mb-1.5 text-title font-semibold">Nothing here</h2>
            <p className="mb-3.5 text-muted-foreground">
              {filtered
                ? filter.text
                  ? `No issues match “${filter.text}”.`
                  : 'No issues match these filters.'
                : rows.length > 0
                  ? `Every issue is in ${closed.map((status) => status.name).join(' or ')}, which are hidden.`
                  : 'There are no issues yet.'}{' '}
              Issues appear here as the runner picks them up and moves them through the workflow.
            </p>
            {!filtered && rows.length > 0 ? (
              <Button
                variant="outline"
                onClick={() => {
                  setHideClosed(false)
                }}
              >
                Show them
              </Button>
            ) : filtered ? (
              <Button
                variant="outline"
                onClick={() => {
                  setLocal({ text: '', who: '' })
                  onSearch({ layout: url.layout })
                }}
              >
                Clear filters
              </Button>
            ) : (
              <Button
                onClick={() => {
                  setCreating(true)
                }}
              >
                New issue
              </Button>
            )}
          </section>
        ) : layout === 'list' ? (
          <RowMenu
            targets={rowsOf(rowMenu.ids)}
            context={pickerContext}
            onTarget={(id) => {
              if (!id) return false
              setRowMenu({ ids: checked.has(id) ? targets() : [id], open: false })
              return true
            }}
            onOpenChange={(open) => {
              setRowMenu((state) => ({ ...state, open }))
            }}
            onApply={(action, edits, untouched) => void bulkEdit(action, edits, untouched)}
          >
            <ListView {...viewProps} checked={checked} onCheck={check} />
          </RowMenu>
        ) : (
          <BoardView
            {...viewProps}
            column={shownColumn}
            onColumn={(key) => {
              setColumn(key)
              setSelected(null)
            }}
          />
        )}
      </main>

      <footer className="sticky bottom-0 z-5 flex flex-wrap items-center gap-x-3 gap-y-1 border-t bg-card px-3 py-1.5 text-caption text-muted-foreground sm:gap-x-4 sm:px-4 sm:py-2 sm:text-meta">
        <span className="font-semibold text-foreground">{layout === 'list' ? 'List' : 'Board'} shortcuts</span>
        {SHORTCUTS[layout].map(([keys, what]) => (
          <span key={what} className="inline-flex items-center gap-1.25 whitespace-nowrap">
            {keys.split(' ').map((key) => (
              <kbd key={key}>{key}</kbd>
            ))}{' '}
            {what}
          </span>
        ))}
      </footer>

      {menu && menuRow && (
        <StatusMenu
          issueId={menu.id}
          current={menuRow.issue.status}
          statuses={statuses}
          open
          onOpenChange={(open) => {
            if (!open) setMenu(null)
          }}
          onChoose={(to) => {
            void move(menu.id, to)
          }}
        >
          <span
            className="pointer-events-none fixed size-0"
            style={{ left: menu.left, top: menu.top }}
            aria-hidden="true"
          />
        </StatusMenu>
      )}

      {picker && (
        <PickerMenu
          kind={picker.kind}
          at={picker}
          targets={rowsOf(picker.ids)}
          context={pickerContext}
          onClose={() => {
            setPicker(null)
          }}
          onApply={(action, edits, untouched) => void bulkEdit(action, edits, untouched)}
        />
      )}

      <IssueSheet
        workspaceId={workspaceId}
        row={sheetRow}
        status={statusByKey(sheetRow?.issue.status)}
        statuses={statuses}
        actors={actors.data ?? []}
        transition={sheet ? transitions[sheet] : undefined}
        now={now}
        onClose={() => {
          setSheet(null)
        }}
        onEdit={edit}
        onMove={(to) => {
          if (sheet) void move(sheet, to)
        }}
        onDismiss={() => {
          if (sheet) dismiss(sheet)
        }}
      />

      <NewIssue
        workspaceId={workspaceId}
        open={creating}
        onOpenChange={setCreating}
        onCreated={(id) => {
          setSelected(id)
        }}
      />
    </div>
  )
}
