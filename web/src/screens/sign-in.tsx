import { useState } from 'react'
import { useQuery } from '@tanstack/react-query'
import { useNavigate, useSearch } from '@tanstack/react-router'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { ApiError } from '@/api/client'
import { signInOptionsQuery, useSignIn } from '@/lib/queries'

/** Sign in with GitHub, when the server has it, or with a token; either starts a session held in an HttpOnly cookie. */
export function SignInScreen() {
  const signIn = useSignIn()
  const navigate = useNavigate()
  const github = useQuery(signInOptionsQuery).data?.github === true
  const { unlinked } = useSearch({ from: '/sign-in' })
  const [token, setToken] = useState('')
  const failed =
    signIn.error instanceof ApiError && signIn.error.status === 401
      ? 'That is not a person’s token.'
      : signIn.error?.message
  return (
    <main className="grid min-h-screen place-items-center px-4">
      <form
        className="grid w-full max-w-90 gap-3 rounded-lg border bg-card p-5"
        onSubmit={(event) => {
          event.preventDefault()
          signIn.mutate(token.trim(), { onSuccess: () => void navigate({ to: '/' }) })
        }}
      >
        <h1 className="text-title font-semibold">Sign in to Punchlist</h1>
        {unlinked && (
          <p className="text-meta text-bad wrap-anywhere" role="alert" data-unlinked>
            No Punchlist account is linked to GitHub user {unlinked}. Sign in with a token once and link GitHub from the
            account menu.
          </p>
        )}
        {github && (
          <>
            {/* A navigation, not a fetch: the server sends the browser on to GitHub. */}
            <Button asChild variant="outline">
              <a href="/api/auth/github/start" data-github-sign-in>
                Sign in with GitHub
              </a>
            </Button>
            <p className="flex items-center gap-2 text-meta text-ink-3 before:h-px before:flex-1 before:bg-border after:h-px after:flex-1 after:bg-border">
              or
            </p>
          </>
        )}
        <p className="text-muted-foreground">
          Paste your token. <code className="font-mono text-meta">punchlist-server bootstrap</code> printed it in your{' '}
          <code className="font-mono text-meta">pl</code> config.
        </p>
        <Input
          type="password"
          aria-label="Token"
          placeholder="Token"
          autoComplete="current-password"
          value={token}
          autoFocus
          onChange={(event) => {
            setToken(event.target.value)
          }}
        />
        {failed && (
          <p className="text-meta text-bad" role="alert">
            {failed}
          </p>
        )}
        <Button type="submit" disabled={!token.trim() || signIn.isPending}>
          {signIn.isPending ? 'Signing in…' : 'Sign in'}
        </Button>
      </form>
    </main>
  )
}
