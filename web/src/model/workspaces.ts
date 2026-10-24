// Which workspace the app shows (ADR 0011), as pure functions over the API's types. No React here.
import type { components } from '@/api/schema'

export type Workspace = components['schemas']['Workspace']

/** The prefix `/` opens: the workspace last used while the account is still in it, else the oldest, else none. */
export function pickWorkspace(workspaces: readonly Workspace[], lastUsed: string | null): string | undefined {
  return (workspaces.find((w) => w.issue_prefix === lastUsed) ?? workspaces[0])?.issue_prefix
}

/** The prefix of an issue id: `GW` for `GW-12`. */
export const issuePrefix = (id: string): string => id.slice(0, Math.max(0, id.lastIndexOf('-')))
