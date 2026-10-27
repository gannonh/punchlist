import { useState } from 'react'
import { useQuery } from '@tanstack/react-query'
import { useNavigate } from '@tanstack/react-router'
import { ChevronDown } from 'lucide-react'
import { cn } from 'cn'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogTitle } from '@/components/ui/dialog'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { sessionQuery, sessionsQuery, signInOptionsQuery, useRevokeSessions, useSignOut } from '@/lib/queries'
import { age } from '@/model/issues'

/**
 * The signed-in person's menu: the GitHub user the account signs in as, or `Link GitHub` when the server has sign-in
 * with GitHub, `Sessions`, and `Sign out`.
 */
export function AccountMenu({ me, className }: { me: string; className?: string }) {
  const github = useQuery(sessionQuery).data?.account?.github_login
  const canLink = useQuery(signInOptionsQuery).data?.github === true
  const signOut = useSignOut()
  const navigate = useNavigate()
  const [sessionsOpen, setSessionsOpen] = useState(false)
  return (
    <>
      <DropdownMenu>
        <DropdownMenuTrigger
          aria-label="Account"
          data-account-menu
          className={cn(
            'inline-flex cursor-pointer items-center gap-1 rounded-sm text-meta text-ink-3 hover:text-foreground focus-visible:outline-2 focus-visible:outline-ring',
            className,
          )}
        >
          {me}
          <ChevronDown className="size-3.5" aria-hidden="true" />
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end" collisionPadding={12}>
          <DropdownMenuLabel className="text-meta font-normal text-ink-3">Signed in as {me}</DropdownMenuLabel>
          {github ? (
            <DropdownMenuLabel className="text-body font-normal" data-github-login={github}>
              GitHub: {github}
            </DropdownMenuLabel>
          ) : (
            canLink && (
              // A navigation, not a fetch: the server sends the browser on to GitHub.
              <DropdownMenuItem asChild className="text-body">
                <a href="/api/auth/github/start?link=1" data-link-github>
                  Link GitHub
                </a>
              </DropdownMenuItem>
            )
          )}
          <DropdownMenuItem
            className="text-body"
            data-sessions-item
            onSelect={() => {
              setSessionsOpen(true)
            }}
          >
            Sessions
          </DropdownMenuItem>
          <DropdownMenuSeparator />
          <DropdownMenuItem
            className="text-body"
            onSelect={() => {
              signOut.mutate(undefined, { onSuccess: () => void navigate({ to: '/sign-in' }) })
            }}
          >
            Sign out
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
      <Dialog open={sessionsOpen} onOpenChange={setSessionsOpen}>
        {sessionsOpen && <SessionsDialog />}
      </Dialog>
    </>
  )
}

/** The browsers the account is signed in on, newest use first, each but this one with its own `Sign out`. */
function SessionsDialog() {
  const sessions = useQuery(sessionsQuery)
  const revoke = useRevokeSessions()
  const now = new Date()
  const others = sessions.data?.filter((s) => !s.current).length ?? 0
  return (
    <DialogContent className="bg-card" data-sessions>
      <DialogTitle className="text-title">Sessions</DialogTitle>
      <DialogDescription className="text-meta text-ink-3">
        The browsers your account is signed in on. Signing one out ends it on its next request.
      </DialogDescription>
      {sessions.isPending ? (
        <p className="text-meta text-ink-3">Loading…</p>
      ) : sessions.isError ? (
        <p className="text-meta text-bad" role="alert">
          Could not read the sessions.
        </p>
      ) : (
        <ul className="grid max-h-[60vh] min-w-0 gap-px overflow-y-auto rounded-md border bg-border">
          {sessions.data.map((s) => (
            <li key={s.id} data-session={s.id} className="flex min-w-0 items-center gap-3 bg-card px-3 py-2.5">
              <div className="min-w-0 flex-1">
                <p className="text-body font-medium wrap-anywhere" data-device>
                  {s.device}
                </p>
                <p className="text-meta text-ink-3">
                  Last used {age(s.last_used_at, now)} ago · signed in{' '}
                  {new Date(s.created_at).toLocaleDateString(undefined, { dateStyle: 'medium' })}
                </p>
              </div>
              {s.current ? (
                <span className="shrink-0 text-meta font-medium text-ink-3" data-current>
                  This device
                </span>
              ) : (
                <Button
                  variant="outline"
                  size="sm"
                  aria-label={`Sign out ${s.device}, last used ${age(s.last_used_at, now)} ago`}
                  onClick={() => {
                    revoke.mutate(s.id)
                  }}
                >
                  Sign out
                </Button>
              )}
            </li>
          ))}
        </ul>
      )}
      <DialogFooter>
        <Button
          variant="outline"
          disabled={others === 0}
          data-sign-out-others
          onClick={() => {
            revoke.mutate('others')
          }}
        >
          Sign out everywhere else
        </Button>
      </DialogFooter>
    </DialogContent>
  )
}
