import { useEffect, type ReactNode } from 'react'
import { Link, useNavigate } from '@tanstack/react-router'
import { WorkspaceMenu } from '@/components/workspace-menu'
import type { Workspace } from '@/model/workspaces'

export const inField = (target: EventTarget | null) =>
  target instanceof HTMLElement &&
  (target.isContentEditable || ['INPUT', 'SELECT', 'TEXTAREA'].includes(target.tagName))

const CHORD_MS = 1000

/** `g` then `p` within a second opens the Pull requests page, except while typing or with a dialog or menu open. */
function useGoToPullRequests(prefix: string) {
  const navigate = useNavigate()
  useEffect(() => {
    let g = -Infinity
    const onKey = (event: KeyboardEvent) => {
      if (event.metaKey || event.ctrlKey || event.altKey || inField(event.target)) return
      if (document.querySelector('[role="dialog"], [role="menu"]')) return
      if (event.key === 'g') g = event.timeStamp
      else if (event.key === 'p' && event.timeStamp - g <= CHORD_MS) {
        event.preventDefault()
        g = -Infinity
        void navigate({ to: '/w/$prefix/pull-requests', params: { prefix } })
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

/** The header of the list, the board and the Pull requests page: the workspace menu, its screens, then the screen's own controls. */
export function AppHeader({
  workspace,
  current,
  pullRequests,
  children,
}: {
  workspace: Workspace
  current: 'issues' | 'pull-requests'
  /** The open pull requests' count, once read. */
  pullRequests: number | undefined
  children?: ReactNode
}) {
  const prefix = workspace.issue_prefix
  useGoToPullRequests(prefix)
  return (
    <header className="sticky top-0 z-5 flex flex-wrap items-center gap-x-3.5 gap-y-2.5 border-b bg-background px-3 py-2 sm:px-4 sm:py-2.5">
      <WorkspaceMenu current={workspace} />
      <nav className="flex items-center gap-3" aria-label="Screens">
        <Link
          to="/w/$prefix"
          params={{ prefix }}
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
      </nav>
      {children}
    </header>
  )
}
