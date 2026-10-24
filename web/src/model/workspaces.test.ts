import { describe, expect, it } from 'vitest'
import { issuePrefix, pickWorkspace, type Workspace } from './workspaces'

const workspace = (issue_prefix: string, name: string): Workspace => ({
  id: `id-${issue_prefix}`,
  name,
  issue_prefix,
  repository: `gannonh/${issue_prefix.toLowerCase()}`,
})
const both = [workspace('PL', 'Punchlist'), workspace('GW', 'Groundwork')]

describe('pickWorkspace', () => {
  it('opens the workspace last used', () => {
    expect(pickWorkspace(both, 'GW')).toBe('GW')
  })
  it('opens the oldest when none was used or the account left the last one', () => {
    expect(pickWorkspace(both, null)).toBe('PL')
    expect(pickWorkspace(both, 'OT')).toBe('PL')
  })
  it('opens none without a workspace', () => {
    expect(pickWorkspace([], 'GW')).toBeUndefined()
  })
})

describe('issuePrefix', () => {
  it('is the id before its number', () => {
    expect(issuePrefix('GW-12')).toBe('GW')
    expect(issuePrefix('PL1-3')).toBe('PL1')
    expect(issuePrefix('nonsense')).toBe('')
  })
})
