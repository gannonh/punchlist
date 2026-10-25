#!/usr/bin/env node
// PL-109, one-off: sets each issue's blockers from the `Blocked by PL-1, PL-3.` lines in the
// workspace's issue bodies, through PATCH /api/issues/{id}. The server does not run it; the
// orchestrator runs it once after the deploy that added `blocked_by`:
//
//   node scripts/backfill-blockers.mjs --workspace PL [--dry-run]
//
// It reads `server_url` and `token` from the pl config (PUNCHLIST_CONFIG, else
// ~/.config/punchlist/config.toml). It adds to the blockers an issue already has, never removes
// one, and prints what it set, each line it could not parse, and each edit the server refused.
// A line parses when, after `Blocked by`, it holds only issue ids, each optionally followed by a
// note in parentheses, separated by commas or `and`, with an optional final period.
import { readFileSync } from 'node:fs'
import { homedir } from 'node:os'
import { join } from 'node:path'

const ID = /^[A-Z][A-Z0-9]*-\d+$/

/** The blockers each `Blocked by` line of `body` names, and the lines that name none plainly. */
export function parseBlockers(body) {
  const ids = []
  const unparsed = []
  for (const line of body.split('\n')) {
    const match = /^\s*(?:[-*]\s+)?blocked by\b(.*)$/i.exec(line)
    if (!match) continue
    const items = match[1]
      .replace(/^\s*:/, '')
      .trim()
      .replace(/\.$/, '')
      .split(/\s*,\s*(?:and\s+)?|\s+and\s+/)
      .map((item) => item.replace(/\s*\([^()]*\)$/, '').trim())
    if (items.length === 0 || !items.every((item) => ID.test(item))) {
      unparsed.push(line.trim())
      continue
    }
    for (const id of items) if (!ids.includes(id)) ids.push(id)
  }
  return { ids, unparsed }
}

function config() {
  const path = process.env.PUNCHLIST_CONFIG ?? join(homedir(), '.config', 'punchlist', 'config.toml')
  const text = readFileSync(path, 'utf8')
  const value = (key) => new RegExp(`^\\s*${key}\\s*=\\s*"([^"]*)"`, 'm').exec(text)?.[1]
  const server = value('server_url')
  const token = value('token')
  if (!server || !token) throw new Error(`${path} needs server_url and token`)
  return { server: server.replace(/\/$/, ''), token }
}

async function main(args) {
  const at = args.indexOf('--workspace')
  const prefix = at >= 0 ? args[at + 1] : undefined
  const dryRun = args.includes('--dry-run')
  if (!prefix) throw new Error('usage: backfill-blockers.mjs --workspace <prefix> [--dry-run]')
  const { server, token } = config()
  const api = async (method, path, body) => {
    const response = await fetch(`${server}${path}`, {
      method,
      headers: { authorization: `Bearer ${token}`, 'content-type': 'application/json' },
      body: body && JSON.stringify(body),
    })
    const answer = await response.json()
    if (!response.ok) throw new Error(answer.message ?? `${method} ${path}: ${response.status}`)
    return answer
  }
  const workspace = (await api('GET', '/api/workspaces')).find(
    (w) => w.issue_prefix.toLowerCase() === prefix.toLowerCase(),
  )
  if (!workspace) throw new Error(`no workspace ${prefix} for this token`)
  const { issues } = await api('GET', `/api/workspaces/${workspace.id}/issues`)
  let failed = 0
  for (const { issue } of issues) {
    const { ids, unparsed } = parseBlockers(issue.body)
    for (const line of unparsed) console.log(`${issue.id}: could not parse: ${line}`)
    const current = (issue.blocked_by ?? []).map((b) => b.id)
    const added = ids.filter((id) => !current.includes(id))
    if (added.length === 0) continue
    if (dryRun) {
      console.log(`${issue.id}: would set blocked by ${added.join(', ')}`)
      continue
    }
    try {
      await api('PATCH', `/api/issues/${issue.id}`, { blocked_by: [...current, ...added] })
      console.log(`${issue.id}: blocked by ${added.join(', ')}`)
    } catch (error) {
      failed += 1
      console.log(`${issue.id}: refused: ${error.message}`)
    }
  }
  if (failed > 0) process.exitCode = 1
}

if (import.meta.url === `file://${process.argv[1]}`) {
  main(process.argv.slice(2)).catch((error) => {
    console.error(`error: ${error.message}`)
    process.exit(1)
  })
}
