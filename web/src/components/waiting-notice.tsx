// An issue waiting on a person, as the issue page and the list's side sheet show it, with Run again for a person.
import { useState } from 'react'
import { Lock } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { useRunAgain } from '@/lib/queries'

/** A person gives an issue that waits on them another set of runs in its status; the inbox's run items have it too. */
export function RunAgain({ id }: { id: string }) {
  const runAgain = useRunAgain(id)
  // Set in the click, so the button is pending from the click on: `isPending` reaches it a render later, which a
  // double click's second click can beat.
  const [sent, setSent] = useState(false)
  return (
    <Button
      variant="outline"
      size="xs"
      className="flex-none self-center"
      data-run-again
      aria-label={`Run again ${id}`}
      disabled={sent}
      aria-busy={sent}
      onClick={() => {
        if (sent) return
        setSent(true)
        runAgain.mutate(undefined, {
          onSettled: () => {
            setSent(false)
          },
        })
      }}
    >
      {sent ? 'Running again…' : 'Run again'}
    </Button>
  )
}

/** `waiting` is the issue's `waiting_on_person`. */
export function WaitingNotice({ id, waiting, person }: { id: string; waiting: string; person: boolean }) {
  return (
    <div className="notice border-warn-line bg-warn-soft text-[#7c3a06]" data-notice="waiting" role="status">
      <Lock className="mt-px size-3.5 flex-none" aria-hidden="true" />
      <div className="min-w-0 flex-1 wrap-anywhere">
        <strong className="text-warn">Waiting on a person:</strong> {waiting}.
      </div>
      {person && <RunAgain id={id} />}
    </div>
  )
}
