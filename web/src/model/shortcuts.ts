/**
 * Each screen's shortcut strip and the keys its key handler reads, both by action id, so a strip button and its key
 * run the screen's one handler for the action (PL-167).
 */

export type StripScreen = 'list' | 'board' | 'inbox' | 'pull-requests' | 'projects' | 'issue'

/**
 * One button of a strip: the keys it shows as written (`⇧p`, `g p`), what it does, and its action. Buttons of one
 * `group` share one item, which reads `group` on a desktop and each button's `what` on a touch screen. `types` marks
 * a button that focuses a field, the only kind that lets the device keyboard open; `toggle` one that holds a mode
 * the strip keeps, standing in for a held Shift.
 */
export interface StripItem {
  keys: string
  what: string
  action: string
  group?: string
  types?: true
  toggle?: true
}

/** The app header's keys, on every screen with it: `i`, and `g` then `p` or `m` within a second. */
export const HEADER_KEYS: Readonly<Record<string, string>> = { i: 'inbox', 'g p': 'pull-requests', 'g m': 'projects' }

/** What the browser does with these keys on the screens of focusable items, which their buttons do to the items. */
const BROWSER_KEYS: Readonly<Record<string, string>> = { Tab: 'next', Enter: 'open' }

const WALK = { j: 'next', ArrowDown: 'next', J: 'next', k: 'previous', ArrowUp: 'previous', K: 'previous' }
const ISSUES = { ...WALK, Enter: 'open', s: 'status', '/': 'filter', '1': 'list', '2': 'board', Escape: 'close' }

/** The keys each screen's handler reads, by `KeyboardEvent.key`. */
export const SCREEN_KEYS: Readonly<Record<StripScreen, Readonly<Record<string, string>>>> = {
  list: { ...ISSUES, x: 'select', a: 'assignee', l: 'labels', P: 'project', M: 'milestone' },
  board: { ...ISSUES, h: 'left', l: 'right' },
  inbox: { Escape: 'issues' },
  'pull-requests': { Escape: 'issues' },
  projects: { Escape: 'issues' },
  issue: { g: 'gates', t: 'activity', c: 'comment', Escape: 'issues' },
}

const LAYOUT: StripItem[] = [
  { keys: '1', what: 'list', action: 'list', group: 'switch layout' },
  { keys: '2', what: 'board', action: 'board', group: 'switch layout' },
]
const SCREENS: StripItem[] = [
  { keys: 'g p', what: 'pull requests', action: 'pull-requests' },
  { keys: 'g m', what: 'projects', action: 'projects' },
]
const PICKS = 'assignee, labels, project, milestone'

export const STRIPS: Readonly<Record<StripScreen, readonly StripItem[]>> = {
  list: [
    { keys: 'j', what: 'next issue', action: 'next' },
    { keys: 'k', what: 'previous issue', action: 'previous' },
    { keys: 'Enter', what: 'open issue', action: 'open' },
    { keys: 's', what: 'change status', action: 'status' },
    { keys: 'x', what: 'select', action: 'select' },
    { keys: '⇧j ⇧k', what: 'select range', action: 'range', toggle: true },
    { keys: 'a', what: 'assignee', action: 'assignee', group: PICKS },
    { keys: 'l', what: 'labels', action: 'labels', group: PICKS },
    { keys: '⇧p', what: 'project', action: 'project', group: PICKS },
    { keys: '⇧m', what: 'milestone', action: 'milestone', group: PICKS },
    { keys: '/', what: 'filter', action: 'filter', types: true },
    ...LAYOUT,
    ...SCREENS,
    { keys: 'Esc', what: 'close', action: 'close' },
  ],
  board: [
    { keys: 'j', what: 'next issue', action: 'next' },
    { keys: 'k', what: 'previous issue', action: 'previous' },
    { keys: 'Enter', what: 'open issue', action: 'open' },
    { keys: 's', what: 'change status', action: 'status' },
    { keys: 'h', what: 'move issue left', action: 'left' },
    { keys: 'l', what: 'move issue right', action: 'right' },
    { keys: '/', what: 'filter', action: 'filter', types: true },
    ...LAYOUT,
    ...SCREENS,
    { keys: 'Esc', what: 'close', action: 'close' },
  ],
  inbox: [
    { keys: 'Tab', what: 'next item', action: 'next' },
    { keys: 'Enter', what: 'open issue', action: 'open' },
    { keys: 'i', what: 'inbox', action: 'inbox' },
    { keys: 'Esc', what: 'issues', action: 'issues' },
  ],
  'pull-requests': [
    { keys: 'Tab', what: 'next pull request', action: 'next' },
    { keys: 'Enter', what: 'open issue or pull request', action: 'open' },
    { keys: 'g p', what: 'pull requests', action: 'pull-requests' },
    { keys: 'Esc', what: 'issues', action: 'issues' },
  ],
  projects: [
    { keys: 'Tab', what: 'next milestone or issue', action: 'next' },
    { keys: 'Enter', what: 'open milestone or issue', action: 'open' },
    { keys: 'g m', what: 'projects', action: 'projects' },
    { keys: 'Esc', what: 'issues', action: 'issues' },
  ],
  issue: [
    { keys: 'g', what: 'gates', action: 'gates' },
    { keys: 't', what: 'activity', action: 'activity' },
    { keys: 'c', what: 'comment', action: 'comment', types: true },
    { keys: 'Esc', what: 'back to the list', action: 'issues' },
  ],
}

/** The action a key as a strip shows it (`⇧p`, `Esc`, `g p`) runs on a screen, through the screen's key handler. */
export function keyAction(screen: StripScreen, shown: string): string | undefined {
  if (screen !== 'issue' && shown in HEADER_KEYS) return HEADER_KEYS[shown]
  const key = shown === 'Esc' ? 'Escape' : shown.replace(/^⇧(.)$/, (_, letter: string) => letter.toUpperCase())
  const browser = screen === 'inbox' || screen === 'pull-requests' || screen === 'projects'
  return SCREEN_KEYS[screen][key] ?? (browser ? BROWSER_KEYS[key] : undefined)
}
