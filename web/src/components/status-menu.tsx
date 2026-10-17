import type { ReactNode } from 'react'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuLabel,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { LockBadge, StatusDot } from '@/components/atoms'
import type { StatusInfo } from '@/model/issues'

/** The prototype's status menu: "Move PL-1 to…", every status in workflow order, the current one checked. */
export function StatusMenu({
  issueId,
  current,
  statuses,
  open,
  onOpenChange,
  onChoose,
  children,
}: {
  issueId: string
  current: string
  statuses: readonly StatusInfo[]
  open?: boolean
  onOpenChange?: (open: boolean) => void
  onChoose: (status: StatusInfo) => void
  children: ReactNode
}) {
  return (
    <DropdownMenu open={open} onOpenChange={onOpenChange}>
      <DropdownMenuTrigger asChild>{children}</DropdownMenuTrigger>
      <DropdownMenuContent
        align="start"
        className="min-w-60"
        aria-label={`Change status of ${issueId}`}
        collisionPadding={12}
      >
        <DropdownMenuLabel className="text-caption font-normal text-ink-3">Move {issueId} to…</DropdownMenuLabel>
        <DropdownMenuRadioGroup
          value={current}
          onValueChange={(key) => {
            const status = statuses.find((s) => s.key === key)
            if (status && key !== current) onChoose(status)
          }}
        >
          {statuses.map((status) => (
            <DropdownMenuRadioItem key={status.key} value={status.key} className="text-body">
              <StatusDot status={status.key} />
              {status.name}
              <span className="ml-auto">
                <LockBadge status={status} />
              </span>
            </DropdownMenuRadioItem>
          ))}
        </DropdownMenuRadioGroup>
      </DropdownMenuContent>
    </DropdownMenu>
  )
}
