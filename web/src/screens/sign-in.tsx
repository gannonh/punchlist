import { useState } from 'react'
import { useNavigate } from '@tanstack/react-router'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { ApiError } from '@/api/client'
import { useSignIn } from '@/lib/queries'

/** The server keeps the token in an HttpOnly session cookie. */
export function SignInScreen() {
  const signIn = useSignIn()
  const navigate = useNavigate()
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
