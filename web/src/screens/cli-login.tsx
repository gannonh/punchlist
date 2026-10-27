import { useQuery } from '@tanstack/react-query'
import { Button } from '@/components/ui/button'
import { cliLoginQuery, useDecideCliLogin } from '@/lib/queries'

const RESULT = {
  approved: (device: string) => `Approved. ${device} is signed in; you can close this tab.`,
  denied: () => 'Denied. pl was not signed in.',
  expired: () => 'This login expired. Run pl auth login again.',
}

/** The approval page `pl auth login` prints: the person checks the code and approves or denies the terminal. */
export function CliLoginScreen({ code, account }: { code: string; account: string }) {
  const login = useQuery(cliLoginQuery(code))
  const decide = useDecideCliLogin(code)
  const decided = decide.isSuccess ? (decide.variables === 'approve' ? 'approved' : 'denied') : undefined
  const state = decided ?? login.data?.state
  const device = login.data?.device ?? 'pl'
  const error = login.error ?? decide.error
  return (
    <main className="grid min-h-screen place-items-center px-4">
      <section className="grid w-full max-w-90 gap-3 rounded-lg border bg-card p-5" aria-labelledby="cli-login-title">
        <h1 id="cli-login-title" className="text-title font-semibold wrap-anywhere">
          {login.data ? `Sign in ${device} as ${account}?` : 'Sign in pl'}
        </h1>
        {login.isPending && <p className="text-muted-foreground">Reading the login…</p>}
        {login.data && (
          <>
            <p className="text-muted-foreground">Check that the terminal shows this code.</p>
            <p className="font-mono text-title tracking-widest" data-code>
              {login.data.code}
            </p>
          </>
        )}
        {state === 'pending' && (
          <div className="grid grid-cols-2 gap-2">
            <Button
              variant="outline"
              disabled={decide.isPending}
              onClick={() => {
                decide.mutate('deny')
              }}
            >
              Deny
            </Button>
            <Button
              disabled={decide.isPending}
              onClick={() => {
                decide.mutate('approve')
              }}
            >
              Approve
            </Button>
          </div>
        )}
        {state && state !== 'pending' && (
          <p role="status" data-result={state}>
            {RESULT[state](device)}
          </p>
        )}
        {error && (
          <p className="text-meta text-bad" role="alert" data-error>
            {error.message}
          </p>
        )}
      </section>
    </main>
  )
}
