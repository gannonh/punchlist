import { Fragment, useState } from 'react'
import { inField } from '@/components/app-header'
import type { StripItem } from '@/model/shortcuts'

/** An action's handler; `range` is a held Shift, or on the strip its `Select range` toggle. */
export type Run = (range: boolean) => void

/**
 * Focuses the next of the screen's items after the focused one, else the first: what `Tab` does, for those items
 * alone.
 */
export function focusNext(selector: string) {
  const items = [...document.querySelectorAll<HTMLElement>(selector)]
  const at = items.findIndex((item) => item.contains(document.activeElement))
  items[Math.min(at + 1, items.length - 1)]?.focus()
}

/** The focused one of the screen's items, which `Enter` acts on. */
export const focused = (selector: string) => document.activeElement?.closest<HTMLElement>(selector) ?? null

const KEYS = (keys: string) => keys.split(' ').map((key) => <kbd key={key}>{key}</kbd>)

/**
 * The shortcut strip at the foot of a screen: each item a button that runs its action's handler, the one its key
 * runs. A button never takes focus, so `Enter` acts on the item `Tab` focused, and one that is not for typing
 * leaves a field, so a tap opens the device keyboard only for the filter and the composer. An action with no handler
 * is disabled, with `why` as its title. On a coarse pointer it scrolls sideways and its buttons are 44 px tall.
 */
export function ShortcutStrip({
  title,
  items,
  actions,
  why,
}: {
  title: string
  items: readonly StripItem[]
  actions: Partial<Record<string, Run>>
  why?: string
}) {
  const [range, setRange] = useState(false)
  const button = (item: StripItem, label: boolean) => {
    const run = actions[item.action]
    return (
      <button
        key={item.action}
        type="button"
        data-action={item.action}
        aria-pressed={item.toggle ? range : undefined}
        disabled={!item.toggle && !run}
        title={!item.toggle && !run ? why : undefined}
        className="inline-flex cursor-pointer items-center gap-1.25 rounded-sm whitespace-nowrap hover:text-foreground focus-visible:outline-2 focus-visible:outline-ring disabled:cursor-default disabled:hover:text-inherit aria-pressed:bg-foreground aria-pressed:px-1 aria-pressed:text-card pointer-coarse:min-h-11 pointer-coarse:rounded-md pointer-coarse:border pointer-coarse:px-2 pointer-coarse:disabled:opacity-50"
        onMouseDown={(event) => {
          event.preventDefault()
        }}
        onClick={() => {
          if (!item.types && inField(document.activeElement)) (document.activeElement as HTMLElement).blur()
          if (item.toggle) setRange((on) => !on)
          else run?.(range)
        }}
      >
        {KEYS(item.keys)}
        {label ? ` ${item.what}` : <span className="hidden pointer-coarse:inline">{item.what}</span>}
      </button>
    )
  }
  // Consecutive items of one group share its text on a desktop.
  const groups = items.reduce<StripItem[][]>((all, item) => {
    const last = all.at(-1)
    if (item.group && last?.[0]?.group === item.group) last.push(item)
    else all.push([item])
    return all
  }, [])
  return (
    <footer
      className="sticky bottom-0 z-5 flex flex-wrap items-center gap-x-3 gap-y-1 border-t bg-card px-3 py-1.5 text-caption text-muted-foreground sm:gap-x-4 sm:px-4 sm:py-2 sm:text-meta pointer-coarse:flex-nowrap pointer-coarse:overflow-x-auto pointer-coarse:pb-[calc(--spacing(1.5)+env(safe-area-inset-bottom))]"
      data-strip
    >
      <span className="font-semibold text-foreground pointer-coarse:whitespace-nowrap">{title}</span>
      {groups.map((group) => {
        const [first] = group
        if (!first) return null
        if (!first.group) return button(first, true)
        return (
          <span key={first.group} className="inline-flex items-center gap-1.25 whitespace-nowrap pointer-coarse:gap-2">
            {group.map((item) => (
              <Fragment key={item.action}>{button(item, false)}</Fragment>
            ))}
            <span className="pointer-coarse:hidden"> {first.group}</span>
          </span>
        )
      })}
    </footer>
  )
}
