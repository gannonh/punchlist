import { useNavigate } from '@tanstack/react-router'
import { ChevronDown } from 'lucide-react'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { useWorkspaces } from '@/lib/queries'
import type { Workspace } from '@/model/workspaces'

/** The current workspace's name and prefix; its menu lists each workspace of the account and opens its list. */
export function WorkspaceMenu({ current }: { current: Workspace }) {
  const workspaces = useWorkspaces().data ?? [current]
  const navigate = useNavigate()
  return (
    <DropdownMenu>
      <DropdownMenuTrigger
        aria-label="Workspace"
        data-workspace-menu={current.issue_prefix}
        className="inline-flex cursor-pointer items-center gap-1.5 rounded-sm text-lead font-semibold focus-visible:outline-2 focus-visible:outline-ring"
      >
        {current.name}
        <span className="font-mono text-meta font-normal text-ink-3">{current.issue_prefix}</span>
        <ChevronDown className="size-3.5 text-muted-foreground" aria-hidden="true" />
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" aria-label="Workspaces" collisionPadding={12}>
        <DropdownMenuRadioGroup
          value={current.issue_prefix}
          onValueChange={(prefix) => void navigate({ to: '/w/$prefix', params: { prefix } })}
        >
          {workspaces.map((workspace) => (
            <DropdownMenuRadioItem
              key={workspace.id}
              value={workspace.issue_prefix}
              className="gap-2 text-body"
              data-workspace={workspace.issue_prefix}
            >
              {workspace.name}
              <span className="ml-auto font-mono text-meta text-ink-3">{workspace.issue_prefix}</span>
            </DropdownMenuRadioItem>
          ))}
        </DropdownMenuRadioGroup>
      </DropdownMenuContent>
    </DropdownMenu>
  )
}
