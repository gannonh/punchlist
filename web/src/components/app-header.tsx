import { useEffect, type ReactNode } from 'react'
import { Link, useNavigate } from '@tanstack/react-router'
import { cn } from 'cn'
import { WorkspaceMenu } from '@/components/workspace-menu'
import { countText } from '@/model/inbox'
import type { Workspace } from '@/model/workspaces'

export const inField = (target: EventTarget | null) =>
  target instanceof HTMLElement &&
  (target.isContentEditable || ['INPUT', 'SELECT', 'TEXTAREA'].includes(target.tagName))

const CHORD_MS = 1000

/**
 * `g` then `p` within a second opens the Pull requests page, `g` then `m` the projects screen, and `i` the inbox,
 * except while typing or with a dialog or menu open.
 */
function useScreenKeys(prefix: string) {
  const navigate = useNavigate()
  useEffect(() => {
    let g = -Infinity
    const onKey = (event: KeyboardEvent) => {
      if (event.metaKey || event.ctrlKey || event.altKey || inField(event.target)) return
      if (document.querySelector('[role="dialog"], [role="menu"]')) return
      if (event.key === 'g') g = event.timeStamp
      else if (event.key === 'i') {
        event.preventDefault()
        g = -Infinity
        void navigate({ to: '/w/$prefix/inbox', params: { prefix } })
      } else if (event.key === 'p' && event.timeStamp - g <= CHORD_MS) {
        event.preventDefault()
        g = -Infinity
        void navigate({ to: '/w/$prefix/pull-requests', params: { prefix } })
      } else if (event.key === 'm' && event.timeStamp - g <= CHORD_MS) {
        event.preventDefault()
        g = -Infinity
        void navigate({ to: '/w/$prefix/projects', params: { prefix } })
      } else g = -Infinity
    }
    document.addEventListener('keydown', onKey)
    return () => {
      document.removeEventListener('keydown', onKey)
    }
  }, [navigate, prefix])
}

const NAV =
  'rounded-sm px-0.5 pb-0.5 text-muted-foreground hover:text-foreground focus-visible:outline-2 focus-visible:outline-ring aria-[current=page]:font-semibold aria-[current=page]:text-foreground aria-[current=page]:shadow-[inset_0_-2px_0_var(--primary)]'

/**
 * The header of the list, the board, the Pull requests page, the inbox and the projects screen: the workspace menu,
 * its screens, then the screen's own controls.
 */
export function AppHeader({
  workspace,
  current,
  pullRequests,
  inbox,
  children,
}: {
  workspace: Workspace
  current: 'issues' | 'pull-requests' | 'inbox' | 'projects'
  /** The open pull requests' count, once read. */
  pullRequests: number | undefined
  /** The inbox's unread count, once read. */
  inbox: number | undefined
  children?: ReactNode
}) {
  const prefix = workspace.issue_prefix
  useScreenKeys(prefix)
  return (
    <header className="sticky top-0 z-5 flex flex-wrap items-center gap-x-3.5 gap-y-2.5 border-b bg-background px-3 py-2 sm:px-4 sm:py-2.5">
      <WorkspaceMenu current={workspace} />
      <nav className="flex items-center gap-3" aria-label="Screens">
        <Link
          to="/w/$prefix"
          params={{ prefix }}
          // Not active on the workspace's other screens, which are under its path.
          activeOptions={{ exact: true }}
          className={NAV}
          aria-current={current === 'issues' ? 'page' : undefined}
        >
          Issues
        </Link>
        <Link
          to="/w/$prefix/pull-requests"
          params={{ prefix }}
          className={`${NAV} inline-flex items-center gap-1.5`}
          aria-current={current === 'pull-requests' ? 'page' : undefined}
        >
          Pull requests
          {pullRequests !== undefined && (
            <span
              className="rounded-full bg-secondary px-1.5 text-caption font-semibold text-muted-foreground"
              data-pull-request-count
            >
              {pullRequests}
            </span>
          )}
        </Link>
        <Link
          to="/w/$prefix/inbox"
          params={{ prefix }}
          className={`${NAV} inline-flex items-center gap-1.5`}
          aria-current={current === 'inbox' ? 'page' : undefined}
        >
          Inbox
          {inbox !== undefined && (
            <span
              className={cn(
                'rounded-full px-1.5 text-caption font-semibold',
                inbox > 0 ? 'bg-warn-soft text-warn' : 'bg-secondary text-muted-foreground',
              )}
              data-inbox-count
              aria-label={`${String(inbox)} unread`}
            >
              {countText(inbox)}
            </span>
          )}
        </Link>
        <Link
          to="/w/$prefix/projects"
          params={{ prefix }}
          className={NAV}
          aria-current={current === 'projects' ? 'page' : undefined}
        >
          Projects
        </Link>
      </nav>
      {children}
    </header>
  )
}
