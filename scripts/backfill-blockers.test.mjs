// node --test scripts/backfill-blockers.test.mjs (pnpm check runs it)
import assert from 'node:assert/strict'
import { test } from 'node:test'
import { parseBlockers } from './backfill-blockers.mjs'

test('a Blocked by line names its ids; anything else is reported', () => {
  const body = [
    'Some spec.',
    'Blocked by PL-1, PL-3.',
    'Blocked by every other issue.',
    '- Blocked by: PL-7 (Done) and PL-3',
    'Not blocked by PL-9.',
    'Blocked by PL-69, Done.',
  ].join('\n')
  assert.deepEqual(parseBlockers(body), {
    ids: ['PL-1', 'PL-3', 'PL-7'],
    unparsed: ['Blocked by every other issue.', 'Blocked by PL-69, Done.'],
  })
  assert.deepEqual(parseBlockers('No relations here.'), { ids: [], unparsed: [] })
  assert.deepEqual(parseBlockers('Blocked by.'), { ids: [], unparsed: ['Blocked by.'] })
})
