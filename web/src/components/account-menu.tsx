import { useQuery } from '@tanstack/react-query'
import { useNavigate } from '@tanstack/react-router'
import { ChevronDown } from 'lucide-react'
import { cn } from 'cn'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { sessionQuery, signInOptionsQuery, useSignOut } from '@/lib/queries'

/**
 * The signed-in person's menu: the GitHub user the account signs in as, or `Link GitHub` when the server has sign-in
 * with GitHub, and `Sign out`.
 */
export function AccountMenu({ me, className }: { me: string; className?: string }) {
  const github = useQuery(sessionQuery).data?.account?.github_login
  const canLink = useQuery(signInOptionsQuery).data?.github === true
  const signOut = useSignOut()
  const navigate = useNavigate()
  return (
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
  )
}
