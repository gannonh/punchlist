import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, useSyncExternalStore } from 'react'
import { useNavigate } from '@tanstack/react-router'
import { Plus } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { BoardView, ListView } from '@/components/issue-views'
import { IssueSheet } from '@/components/issue-sheet'
import { NewIssue } from '@/components/new-issue'
import { StatusMenu } from '@/components/status-menu'
import {
  AGENT_NAMES,
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
  useEditIssue,
  useEventStream,
  useIssues,
  useSignOut,
  useTransitions,
  useWorkflow,
} from '@/lib/queries'

export type Layout = 'list' | 'board'

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

const inField = (target: EventTarget | null) =>
  target instanceof HTMLElement &&
  (target.isContentEditable || ['INPUT', 'SELECT', 'TEXTAREA'].includes(target.tagName))

const SHORTCUTS: Record<Layout, [string, string][]> = {
  list: [
    ['j', 'next issue'],
    ['k', 'previous issue'],
    ['Enter', 'open issue'],
    ['s', 'change status'],
    ['/', 'filter'],
    ['1 2', 'switch layout'],
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
    ['Esc', 'close'],
  ],
}

function useNow(): Date {
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
  workspaceId,
  me,
  layout,
  onLayout,
}: {
  workspaceId: string
  me: string
  layout: Layout
  onLayout: (layout: Layout) => void
}) {
  const issues = useIssues()
  const workflow = useWorkflow()
  const actors = useActors()
  const edit = useEditIssue()
  const signOut = useSignOut()
  const navigate = useNavigate()
  const { transitions, request, dismiss } = useTransitions()
  useEventStream(workspaceId, issues.isSuccess)

  const [filter, setFilter] = useState<Filter>({ text: '', who: '' })
  const [selected, setSelected] = useState<string | null>(null)
  const [column, setColumn] = useState<string | null>(null)
  const [sheet, setSheet] = useState<string | null>(null)
  const [menu, setMenu] = useState<{ id: string; left: number; top: number } | null>(null)
  const [creating, setCreating] = useState(false)
  const [hideClosed, setHideClosed] = useState(true)
  const search = useRef<HTMLInputElement>(null)
  const narrow = useNarrow()

  const statuses = useMemo(() => workflow.data?.statuses ?? [], [workflow.data])
  const rows = useMemo(() => issues.data?.rows ?? [], [issues.data])
  const closed = useMemo(() => statuses.filter((status) => status.closed), [statuses])
  const visible = useMemo(() => {
    const shown = filterRows(rows, filter)
    return hideClosed ? shown.filter((row) => !closed.some((status) => status.key === row.issue.status)) : shown
  }, [rows, filter, hideClosed, closed])
  const groups = useMemo(() => groupByStatus(visible, statuses), [visible, statuses])
  const shownColumn = column ?? groups.find((g) => g.rows.length > 0)?.status.key ?? statuses[0]?.key ?? ''
  const order = useMemo(() => navOrder(groups, layout, narrow, shownColumn), [groups, layout, narrow, shownColumn])
  const current = selected !== null && order.includes(selected) ? selected : (order[0] ?? null)
  const rowById = useCallback((id: string | null) => rows.find((row) => row.issue.id === id), [rows])
  const statusByKey = useCallback((key: string | undefined) => statuses.find((s) => s.key === key), [statuses])
  const now = useNow()
  const currentRef = useRef(current)
  useLayoutEffect(() => {
    currentRef.current = current
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

  const openMenu = useCallback((id: string) => {
    const rect = document.querySelector(`[data-issue="${id}"]`)?.getBoundingClientRect()
    setMenu({ id, left: (rect?.left ?? 20) + 12, top: rect?.bottom ?? 80 })
  }, [])

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.metaKey || event.ctrlKey || event.altKey) return
      if (inField(event.target)) {
        if (event.key === 'Escape') (event.target as HTMLElement).blur()
        return
      }
      if (sheet || menu || creating) return
      const step = (delta: number) => {
        if (order.length === 0) return
        const index = Math.max(0, current ? order.indexOf(current) : 0)
        setSelected(order[Math.min(order.length - 1, Math.max(0, index + delta))] ?? null)
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
          if (current) setSheet(current)
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
  }, [sheet, menu, creating, order, current, layout, statuses, transitions, rowById, move, openMenu, onLayout])

  const filtered = filter.text !== '' || filter.who !== ''
  const menuRow = rowById(menu?.id ?? null)
  const sheetRow = rowById(sheet)
  const viewProps = { groups, selected: current, transitions, now, onPick: pick, onDismiss: dismiss }

  return (
    <div className="flex min-h-screen flex-col" data-last-seq={issues.data?.lastSeq}>
      <header className="sticky top-0 z-5 flex flex-wrap items-center gap-x-3.5 gap-y-2.5 border-b bg-background px-3 py-2 sm:px-4 sm:py-2.5">
        <div className="flex items-center gap-2 text-lead font-semibold">Punchlist</div>
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
            value={filter.text}
            className="h-7.5 flex-[1_1_120px] rounded-md border bg-card px-2 py-1.25 focus:outline-2 focus:outline-offset-1 focus:outline-ring sm:w-45 sm:flex-none"
            onChange={(event) => {
              setFilter((f) => ({ ...f, text: event.target.value }))
            }}
          />
          <select
            aria-label="Assignee filter"
            value={filter.who}
            className="h-7.5 rounded-md border bg-card px-2 focus:outline-2 focus:outline-offset-1 focus:outline-ring"
            onChange={(event) => {
              setFilter((f) => ({ ...f, who: event.target.value as Who }))
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
          <Button
            size="sm"
            className="h-7.5"
            onClick={() => {
              setCreating(true)
            }}
          >
            <Plus /> New issue
          </Button>
          <button
            type="button"
            className="cursor-pointer text-meta text-ink-3 hover:text-foreground"
            title={`Signed in as ${me}`}
            onClick={() => {
              signOut.mutate(undefined, { onSuccess: () => void navigate({ to: '/sign-in' }) })
            }}
          >
            Sign out
          </button>
        </div>
      </header>

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
                  setFilter({ text: '', who: '' })
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
          <ListView {...viewProps} />
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

      <IssueSheet
        row={sheetRow}
        status={statusByKey(sheetRow?.issue.status)}
        statuses={statuses}
        actors={actors.data ?? []}
        transition={sheet ? transitions[sheet] : undefined}
        now={now}
        onClose={() => {
          setSheet(null)
        }}
        onEdit={(change) => {
          edit.mutate(change)
        }}
        onMove={(to) => {
          if (sheet) void move(sheet, to)
        }}
        onDismiss={() => {
          if (sheet) dismiss(sheet)
        }}
      />

      <NewIssue
        open={creating}
        onOpenChange={setCreating}
        onCreated={(id) => {
          setSelected(id)
        }}
      />
    </div>
  )
}
