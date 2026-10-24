// Server state (ADR 0006): one cached issue list kept current by the event stream, optimistic field edits,
// and transitions that wait for the server.
import { queryOptions, useMutation, useQuery, useQueryClient, type QueryClient } from '@tanstack/react-query'
import { useCallback, useEffect, useRef, useState } from 'react'
import { toast } from 'sonner'
import { api, ApiError, call } from '@/api/client'
import {
  applyMove,
  applyStream,
  applyWrite,
  fromList,
  refusalText,
  replaceRow,
  revertEdit,
  type ActorInfo,
  type IssueCache,
  type IssueRow,
  type MilestoneRef,
  type ProjectRef,
  type SearchResults,
  type StatusInfo,
  type StreamItem,
  type Transitions,
  workflowStale,
} from '@/model/issues'
import { proofType, type GateChecklist } from '@/model/issue-page'
import type { InboxItem } from '@/model/inbox'

export const sessionQuery = queryOptions({
  queryKey: ['session'],
  queryFn: async () => {
    try {
      return await call(api.GET('/api/session'))
    } catch (error) {
      if (error instanceof ApiError && error.status === 401) return null
      throw error
    }
  },
  staleTime: Infinity,
})

/** The workspaces the signed-in account belongs to, oldest first (ADR 0011). */
export const workspacesQuery = queryOptions({
  queryKey: ['workspaces'],
  queryFn: () => call(api.GET('/api/workspaces')),
  staleTime: 60_000,
})

// Each workspace's reads are keyed by its id and call its `/api/workspaces/{id}/` routes, so switching shows the
// other workspace's cache and never mixes their rows.
const inWorkspace = (id: string) => ({ params: { path: { id } } })

export const workflowQuery = (workspaceId: string) =>
  queryOptions({
    queryKey: ['workflow', workspaceId],
    queryFn: () => call(api.GET('/api/workspaces/{id}/workflow', inWorkspace(workspaceId))),
    staleTime: 60_000,
  })

export const actorsQuery = (workspaceId: string) =>
  queryOptions({
    queryKey: ['actors', workspaceId],
    queryFn: async () => (await call(api.GET('/api/workspaces/{id}/actors', inWorkspace(workspaceId)))).actors,
    staleTime: 60_000,
  })

// Read again each time the sheet opens, so a project made with `pl` meanwhile is there to pick.
export const projectsQuery = (workspaceId: string) =>
  queryOptions({
    queryKey: ['projects', workspaceId],
    queryFn: async () => (await call(api.GET('/api/workspaces/{id}/projects', inWorkspace(workspaceId)))).projects,
  })

// The stream keeps the list current, so it never goes stale on its own.
export const issuesQuery = (workspaceId: string) =>
  queryOptions({
    queryKey: ['issues', workspaceId],
    queryFn: async () => fromList(await call(api.GET('/api/workspaces/{id}/issues', inWorkspace(workspaceId)))),
    staleTime: Infinity,
  })

// Read again on a `pull_request`, `transition` or `workflow` event, and after the stream reopens.
export const pullRequestsQuery = (workspaceId: string) =>
  queryOptions({
    queryKey: ['pull-requests', workspaceId],
    queryFn: () => call(api.GET('/api/workspaces/{id}/pull-requests', inWorkspace(workspaceId))),
    staleTime: Infinity,
  })

// The kinds that can change a row of the Pull requests page or move it between sections.
const PULL_REQUEST_KINDS: ReadonlySet<string> = new Set(['pull_request', 'transition', 'workflow'])

// What waits on the signed-in person: read again 100 ms after the last event that can add, remove or change an item.
export const inboxQuery = (workspaceId: string) =>
  queryOptions({
    queryKey: ['inbox', workspaceId],
    queryFn: () => call(api.GET('/api/workspaces/{id}/inbox', inWorkspace(workspaceId))),
    staleTime: Infinity,
  })

// A move or a new issue changes which issues wait; an edit can change the assignee, so whose inbox an issue is in.
const INBOX_KINDS: ReadonlySet<string> = new Set(['transition', 'created', 'edited', 'run', 'workflow'])

const setIssues = (client: QueryClient, workspaceId: string, change: (cache: IssueCache) => IssueCache) => {
  client.setQueryData(issuesQuery(workspaceId).queryKey, (cache) => (cache ? change(cache) : cache))
}

/**
 * Opens `GET /api/workspaces/{id}/events` once the workspace's list is loaded, after the list's last sequence. A
 * new workspace closes the old one's stream and opens its own.
 */
export function useEventStream(workspaceId: string, ready: boolean): void {
  const client = useQueryClient()
  const [opened, setOpened] = useState(0)
  const failures = useRef(0)
  useEffect(() => {
    if (!ready) return
    let retry: ReturnType<typeof setTimeout> | undefined
    let reread: ReturnType<typeof setTimeout> | undefined
    let rereadInbox: ReturnType<typeof setTimeout> | undefined
    const issues = issuesQuery(workspaceId).queryKey
    const workflow = workflowQuery(workspaceId).queryKey
    const pullRequests = pullRequestsQuery(workspaceId).queryKey
    const inbox = inboxQuery(workspaceId).queryKey
    const after = client.getQueryData(issues)?.lastSeq ?? 0
    // EventSource reconnects by itself after a network error and sends Last-Event-ID, which the server prefers
    // over `after`.
    const source = new EventSource(`/api/workspaces/${workspaceId}/events?after=${String(after)}`)
    source.addEventListener('issue', (message) => {
      const item = JSON.parse((message as MessageEvent<string>).data) as StreamItem
      failures.current = 0
      setIssues(client, workspaceId, (cache) => applyStream(cache, item))
      // An open issue page reads its issue again: its timeline, runs, proof, gates and moves.
      void client.invalidateQueries({ queryKey: issueKey(item.event.issue_id) })
      if (PULL_REQUEST_KINDS.has(item.event.detail.kind))
        void client.invalidateQueries({ queryKey: pullRequests })
      // One read per burst of events, as for the workflow below.
      if (INBOX_KINDS.has(item.event.detail.kind)) {
        clearTimeout(rereadInbox)
        rereadInbox = setTimeout(() => void client.invalidateQueries({ queryKey: inbox }), 100)
      }
      // A load writes one `workflow` message per issue. One read starts 100 ms after the last of them, so it
      // starts after every load they came from committed.
      if (workflowStale(item, client.getQueryData(workflow)?.statuses)) {
        clearTimeout(reread)
        reread = setTimeout(() => void client.invalidateQueries({ queryKey: workflow }), 100)
      }
    })
    // An HTTP error closes it for good, such as a proxy's 502 while the server restarts or a 401 once the session
    // ends. Read the session, a fresh list and the workflow, then open it again; a signed-out session shows the sign-in page.
    // The wait doubles from 2 s to at most 60 s while the stream keeps closing, and resets once an event arrives.
    source.addEventListener('error', () => {
      if (source.readyState !== EventSource.CLOSED) return
      const wait = Math.min(2000 * 2 ** failures.current, 60_000)
      failures.current += 1
      retry = setTimeout(() => {
        void Promise.allSettled([
          client.refetchQueries({ queryKey: sessionQuery.queryKey }),
          client.refetchQueries({ queryKey: issues }),
          client.refetchQueries({ queryKey: workflow }),
          client.invalidateQueries({ queryKey: pullRequests }),
          client.invalidateQueries({ queryKey: inbox }),
        ]).then(() => {
          setOpened((count) => count + 1)
        })
      }, wait)
    })
    return () => {
      clearTimeout(retry)
      clearTimeout(reread)
      clearTimeout(rereadInbox)
      source.close()
    }
  }, [client, workspaceId, ready, opened])
}

export function useSignIn() {
  const client = useQueryClient()
  return useMutation({
    mutationFn: (token: string) => call(api.POST('/api/session', { body: { token } })),
    onSuccess: (session) => {
      client.setQueryData(sessionQuery.queryKey, session)
    },
  })
}

export function useSignOut() {
  const client = useQueryClient()
  return useMutation({
    mutationFn: () => call(api.DELETE('/api/session')),
    onSuccess: () => {
      client.clear()
      client.setQueryData(sessionQuery.queryKey, null)
    },
  })
}

export function useCreateIssue(workspaceId: string) {
  const client = useQueryClient()
  return useMutation({
    mutationFn: (body: { title: string; body: string }) =>
      call(api.POST('/api/workspaces/{id}/issues', { ...inWorkspace(workspaceId), body })),
    onSuccess: (written) => {
      setIssues(client, workspaceId, (cache) => applyWrite(cache, written))
    },
    onError: (error) => {
      toast.error(error instanceof ApiError ? refusalText(error.body) : String(error))
    },
  })
}

/**
 * A field edit. `assignee`, `project` and `milestone` are the things themselves, so the row can show them before
 * the server answers.
 */
export interface Edit {
  id: string
  title?: string
  labels?: string[]
  assignee?: ActorInfo | null
  parent?: string | null
  project?: ProjectRef | null
  milestone?: MilestoneRef | null
}

export function useEditIssue(workspaceId: string) {
  const client = useQueryClient()
  return useMutation({
    // Requests go one at a time in the order they were made, each shown at once: a milestone chosen while its
    // project's edit is on its way reaches the server after that edit, which it is checked against.
    scope: { id: 'edit-issue' },
    mutationFn: ({ id, assignee, project, milestone, ...fields }: Edit) =>
      call(
        api.PATCH('/api/issues/{id}', {
          params: { path: { id } },
          body: {
            ...fields,
            ...(assignee !== undefined && { assignee: assignee?.id ?? null }),
            ...(project !== undefined && { project: project?.id ?? null }),
            ...(milestone !== undefined && { milestone: milestone?.id ?? null }),
          },
        }),
      ),
    onMutate: async (
      edit,
    ): Promise<{ edited: IssueRow['issue']; seq: number } | undefined> => {
      await client.cancelQueries({ queryKey: issuesQuery(workspaceId).queryKey })
      const cache = client.getQueryData(issuesQuery(workspaceId).queryKey)
      const before = cache?.rows.find((row) => row.issue.id === edit.id)?.issue
      if (!cache || !before) return undefined
      const edited: IssueRow['issue'] = {
        ...before,
        ...(edit.title !== undefined && { title: edit.title }),
        ...(edit.labels !== undefined && { labels: edit.labels }),
        ...(edit.assignee !== undefined && {
          assignee: edit.assignee && {
            id: edit.assignee.id,
            name: edit.assignee.name,
            role: edit.assignee.role,
            kind: edit.assignee.kind,
          },
        }),
        ...(edit.parent !== undefined && { parent: edit.parent }),
        ...(edit.project !== undefined && { project: edit.project }),
        ...(edit.milestone !== undefined && { milestone: edit.milestone }),
      }
      setIssues(client, workspaceId, (cache) => replaceRow(cache, edit.id, (row) => ({ ...row, issue: edited })))
      return { edited, seq: cache.rowSeq[edit.id] ?? -1 }
    },
    onError: (error, edit, context) => {
      if (context) {
        const fields = (['title', 'labels', 'assignee', 'parent', 'project', 'milestone'] as const).filter(
          (field) => edit[field] !== undefined,
        )
        setIssues(client, workspaceId, (cache) => revertEdit(cache, context.edited, fields, context.seq))
      }
      toast.error(`${edit.id} was not changed. ${error instanceof ApiError ? refusalText(error.body) : String(error)}`)
    },
    onSuccess: (written) => {
      setIssues(client, workspaceId, (cache) => applyWrite(cache, written))
    },
  })
}

/** Transitions are not optimistic: the row shows the request as pending, then settles or shows the refusal. */
export function useTransitions(workspaceId: string) {
  const client = useQueryClient()
  const [transitions, setTransitions] = useState<Transitions>({})
  const set = useCallback((id: string, next: Transitions[string] | undefined) => {
    setTransitions((all) => {
      const rest = Object.fromEntries(Object.entries(all).filter(([key]) => key !== id))
      return next ? { ...rest, [id]: next } : rest
    })
  }, [])

  const request = useCallback(
    async (id: string, to: StatusInfo): Promise<boolean> => {
      set(id, { state: 'pending', to: to.key, toName: to.name })
      try {
        const moved = await call(
          api.POST('/api/issues/{id}/transitions', { params: { path: { id } }, body: { to: to.key } }),
        )
        setIssues(client, workspaceId, (cache) => applyMove(cache, moved))
        set(id, undefined)
        if (moved.warning) toast.warning(moved.warning)
        return true
      } catch (error) {
        const text = error instanceof ApiError ? refusalText(error.body) : String(error)
        set(id, { state: 'refused', to: to.key, toName: to.name, text })
        return false
      }
    },
    [client, set, workspaceId],
  )

  const dismiss = useCallback(
    (id: string) => {
      set(id, undefined)
    },
    [set],
  )

  return { transitions, request, dismiss }
}

export const useWorkspaces = () => useQuery(workspacesQuery)
export const useIssues = (workspaceId: string) => useQuery(issuesQuery(workspaceId))
export const useWorkflow = (workspaceId: string) => useQuery(workflowQuery(workspaceId))
export const useActors = (workspaceId: string) => useQuery(actorsQuery(workspaceId))
export const useProjects = (workspaceId: string) => useQuery(projectsQuery(workspaceId))
/**
 * The workspace's open pull requests, read once `ready`: after `GET /api/issues` has succeeded, so the header's
 * count never delays the list, and the stream, opened after the list's last event, reads them again on any later
 * change.
 */
export const usePullRequests = (workspaceId: string, ready: boolean) =>
  useQuery({ ...pullRequestsQuery(workspaceId), enabled: ready })

/** What waits on the signed-in person, read once `ready`, as `usePullRequests` is. */
export const useInbox = (workspaceId: string, ready: boolean) =>
  useQuery({ ...inboxQuery(workspaceId), enabled: ready })

/** Opening an inbox item marks it read up to its event; the inbox is read again after. */
export function useMarkRead(workspaceId: string) {
  const client = useQueryClient()
  return useMutation({
    mutationFn: (item: InboxItem) =>
      call(
        api.POST('/api/workspaces/{id}/inbox/read', {
          ...inWorkspace(workspaceId),
          body: { issue: item.issue.id, kind: item.kind, since_seq: item.since_seq },
        }),
      ),
    onSettled: () => client.invalidateQueries({ queryKey: inboxQuery(workspaceId).queryKey }),
  })
}

/** The filter's text as the server reads it: trimmed, each whitespace run one space. */
const searchText = (text: string) => text.trim().split(/\s+/).join(' ')
// In code points, as the server counts.
const searchable = (q: string) => Array.from(q).length >= 3 && Array.from(q).length <= 200

/**
 * `GET /api/search` for the filter's text, sent 150 ms after the last keystroke and only from 3 characters. While
 * the next answer loads it keeps the last one; under 3 characters it is `undefined` at once. Each text is its own
 * query, so an earlier answer never replaces a later one.
 */
export function useSearch(workspaceId: string, text: string): SearchResults | undefined {
  const q = searchText(text)
  const [debounced, setDebounced] = useState(q)
  useEffect(() => {
    const timer = setTimeout(() => {
      setDebounced(q)
    }, 150)
    return () => {
      clearTimeout(timer)
    }
  }, [q])
  const search = useQuery({
    queryKey: ['search', workspaceId, debounced],
    queryFn: ({ signal }) =>
      call(
        api.GET('/api/workspaces/{id}/search', {
          params: { path: { id: workspaceId }, query: { q: debounced } },
          signal,
        }),
      ),
    enabled: searchable(debounced),
    // Not across an emptied filter: hits from before it was cleared would show for the next text's first 150 ms.
    placeholderData: (previous) => (searchable(q) ? previous : undefined),
    staleTime: 30_000,
  })
  return searchable(q) ? search.data : undefined
}

/** Every query of one issue's page starts with this key, so one invalidation reads the page again. */
export const issueKey = (id: string) => ['issue', id] as const
const path = (id: string) => ({ params: { path: { id } } })

/** The issue page's reads. Each is an issue route, which a standalone agent's token reaches too. */
export const issuePageQueries = (id: string) => ({
  issue: queryOptions({
    queryKey: [...issueKey(id), 'issue'],
    queryFn: () => call(api.GET('/api/issues/{id}', path(id))),
  }),
  events: queryOptions({
    queryKey: [...issueKey(id), 'events'],
    queryFn: () => call(api.GET('/api/issues/{id}/events', path(id))),
  }),
  runs: queryOptions({
    queryKey: [...issueKey(id), 'runs'],
    queryFn: () => call(api.GET('/api/issues/{id}/runs', path(id))),
  }),
  proof: queryOptions({
    queryKey: [...issueKey(id), 'proof'],
    queryFn: () => call(api.GET('/api/issues/{id}/proof', path(id))),
  }),
  // 204 when no move from the status has gates.
  gates: queryOptions({
    queryKey: [...issueKey(id), 'gates'],
    queryFn: async () => (await call<GateChecklist | undefined>(api.GET('/api/issues/{id}/gates', path(id)))) ?? null,
  }),
  transitions: queryOptions({
    queryKey: [...issueKey(id), 'transitions'],
    queryFn: () => call(api.GET('/api/issues/{id}/transitions', path(id))),
  }),
  pullRequests: queryOptions({
    queryKey: [...issueKey(id), 'pull-requests'],
    queryFn: () => call(api.GET('/api/issues/{id}/pull-requests', path(id))),
  }),
  // As recorded: reads no GitHub, so the stream's invalidation can read it again on every event.
  reviewThreads: queryOptions({
    queryKey: [...issueKey(id), 'review-threads'],
    queryFn: () => call(api.GET('/api/issues/{id}/review-threads', path(id))),
  }),
})

/**
 * A run's whole log. People only: an agent's token cannot read `/api/runs/{id}/log`.
 * ponytail: a running attempt's log is read again in full every second; past a few thousand lines read only
 * the new ones, with an `after` cursor on the route or a log stream over Server-Sent Events.
 */
export const runLogQuery = (issueId: string, runId: string, live: boolean) =>
  queryOptions({
    queryKey: [...issueKey(issueId), 'log', runId],
    queryFn: async () => (await call(api.GET('/api/runs/{id}/log', path(runId)))).lines,
    refetchInterval: live ? 1000 : false,
  })

const refusal = (error: unknown) => (error instanceof ApiError ? refusalText(error.body) : String(error))

/** A comment from the composer; the timeline is read again as soon as it is written. */
export function useComment(id: string) {
  const client = useQueryClient()
  return useMutation({
    mutationFn: (body: string) => call(api.POST('/api/issues/{id}/comments', { ...path(id), body: { body } })),
    onSuccess: () => client.invalidateQueries({ queryKey: issueKey(id) }),
    onError: (error) => {
      toast.error(`The comment was not posted. ${refusal(error)}`)
    },
  })
}

/** A file attached as proof, sent as `multipart/form-data`; the server takes its kind from its type. */
export function useAttachProof(id: string) {
  const client = useQueryClient()
  return useMutation({
    mutationFn: (file: File) => {
      const form = new FormData()
      form.append('title', file.name)
      // The browser's type for a file can be empty (`.log`) or other than the server's; send the server's.
      const type = proofType(file.name)
      form.append('file', type ? file.slice(0, file.size, type) : file, file.name)
      return call(
        api.POST('/api/issues/{id}/proof', {
          ...path(id),
          // openapi-fetch types a binary part as a string; the form carries the file itself.
          body: { title: file.name, file: '' },
          bodySerializer: () => form,
        }),
      )
    },
    onSuccess: () => client.invalidateQueries({ queryKey: issueKey(id) }),
    onError: (error) => {
      toast.error(`The file was not attached. ${refusal(error)}`)
    },
  })
}

/**
 * `Read from GitHub`: the server reads the issue's pull requests and review threads from GitHub, since a new thread
 * sends no event, then the page reads them again. `refreshed` is false when GitHub could not be read.
 */
export function useReadFromGithub(id: string) {
  const client = useQueryClient()
  return useMutation({
    mutationFn: () => call(api.POST('/api/issues/{id}/pull-requests/refresh', path(id))),
    onSuccess: () => client.invalidateQueries({ queryKey: issueKey(id) }),
    onError: (error) => {
      toast.error(`GitHub was not read. ${refusal(error)}`)
    },
  })
}

/** A move requested from the issue page. Not optimistic: the page reads the issue again once it settles. */
export function useMove(id: string, workspaceId: string) {
  const client = useQueryClient()
  return useMutation({
    mutationFn: (to: StatusInfo) =>
      call(api.POST('/api/issues/{id}/transitions', { ...path(id), body: { to: to.key } })),
    onSuccess: (moved) => {
      setIssues(client, workspaceId, (cache) => applyMove(cache, moved))
      if (moved.warning) toast.warning(moved.warning)
    },
    onError: (error, to) => {
      toast.error(`${id} did not move to ${to.name}. ${refusal(error)}`)
    },
    onSettled: () => client.invalidateQueries({ queryKey: issueKey(id) }),
  })
}
