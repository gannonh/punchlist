import { useState } from 'react'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogFooter, DialogTitle } from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import { useCreateIssue } from '@/lib/queries'

export function NewIssue({
  workspaceId,
  open,
  onOpenChange,
  onCreated,
}: {
  workspaceId: string
  open: boolean
  onOpenChange: (open: boolean) => void
  onCreated: (id: string) => void
}) {
  const create = useCreateIssue(workspaceId)
  const [title, setTitle] = useState('')
  const [body, setBody] = useState('')
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="bg-card">
        <DialogTitle className="text-title">New issue</DialogTitle>
        <form
          className="grid gap-3"
          onSubmit={(event) => {
            event.preventDefault()
            if (!title.trim()) return
            create.mutate(
              { title: title.trim(), body },
              {
                onSuccess: (written) => {
                  setTitle('')
                  setBody('')
                  onOpenChange(false)
                  onCreated(written.row.issue.id)
                },
              },
            )
          }}
        >
          <Input
            aria-label="Title"
            placeholder="Title"
            value={title}
            autoFocus
            onChange={(event) => {
              setTitle(event.target.value)
            }}
          />
          <textarea
            aria-label="Body"
            placeholder="What should be done, in Markdown"
            rows={6}
            value={body}
            className="rounded-md border border-input bg-transparent px-3 py-2 shadow-xs outline-none focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring/50"
            onChange={(event) => {
              setBody(event.target.value)
            }}
          />
          <DialogFooter>
            <Button type="submit" disabled={!title.trim() || create.isPending}>
              {create.isPending ? 'Creating…' : 'Create issue'}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  )
}
