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
  type StatusInfo,
  type StreamItem,
  type Transitions,
  workflowStale,
} from '@/model/issues'

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

export const workflowQuery = queryOptions({
  queryKey: ['workflow'],
  queryFn: () => call(api.GET('/api/workflow')),
  staleTime: 60_000,
})

export const actorsQuery = queryOptions({
  queryKey: ['actors'],
  queryFn: async () => (await call(api.GET('/api/actors'))).actors,
  staleTime: 60_000,
})

// Read again each time the sheet opens, so a project made with `pl` meanwhile is there to pick.
export const projectsQuery = queryOptions({
  queryKey: ['projects'],
  queryFn: async () => (await call(api.GET('/api/projects'))).projects,
})

// The stream keeps the list current, so it never goes stale on its own.
export const issuesQuery = queryOptions({
  queryKey: ['issues'],
  queryFn: async () => fromList(await call(api.GET('/api/issues'))),
  staleTime: Infinity,
})

const setIssues = (client: QueryClient, change: (cache: IssueCache) => IssueCache) => {
  client.setQueryData(issuesQuery.queryKey, (cache) => (cache ? change(cache) : cache))
}

/** Opens `GET /api/workspaces/{id}/events` once the list is loaded, after the list's last sequence. */
export function useEventStream(workspaceId: string, ready: boolean): void {
  const client = useQueryClient()
  const [opened, setOpened] = useState(0)
  const failures = useRef(0)
  useEffect(() => {
    if (!ready) return
    let retry: ReturnType<typeof setTimeout> | undefined
    const after = client.getQueryData(issuesQuery.queryKey)?.lastSeq ?? 0
    // EventSource reconnects by itself after a network error and sends Last-Event-ID, which the server prefers
    // over `after`.
    const source = new EventSource(`/api/workspaces/${workspaceId}/events?after=${String(after)}`)
    source.addEventListener('issue', (message) => {
      const item = JSON.parse((message as MessageEvent<string>).data) as StreamItem
      failures.current = 0
      setIssues(client, (cache) => applyStream(cache, item))
      // A load writes one `workflow` message per issue; they share one read of the workflow.
      if (workflowStale(item, client.getQueryData(workflowQuery.queryKey)?.statuses)) {
        void client.invalidateQueries({ queryKey: workflowQuery.queryKey }, { cancelRefetch: false })
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
          client.refetchQueries({ queryKey: issuesQuery.queryKey }),
          client.refetchQueries({ queryKey: workflowQuery.queryKey }),
        ]).then(() => {
          setOpened((count) => count + 1)
        })
      }, wait)
    })
    return () => {
      clearTimeout(retry)
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

export function useCreateIssue() {
  const client = useQueryClient()
  return useMutation({
    mutationFn: (body: { title: string; body: string }) => call(api.POST('/api/issues', { body })),
    onSuccess: (written) => {
      setIssues(client, (cache) => applyWrite(cache, written))
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

export function useEditIssue() {
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
      await client.cancelQueries({ queryKey: issuesQuery.queryKey })
      const cache = client.getQueryData(issuesQuery.queryKey)
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
      setIssues(client, (cache) => replaceRow(cache, edit.id, (row) => ({ ...row, issue: edited })))
      return { edited, seq: cache.rowSeq[edit.id] ?? -1 }
    },
    onError: (error, edit, context) => {
      if (context) {
        const fields = (['title', 'labels', 'assignee', 'parent', 'project', 'milestone'] as const).filter(
          (field) => edit[field] !== undefined,
        )
        setIssues(client, (cache) => revertEdit(cache, context.edited, fields, context.seq))
      }
      toast.error(`${edit.id} was not changed. ${error instanceof ApiError ? refusalText(error.body) : String(error)}`)
    },
    onSuccess: (written) => {
      setIssues(client, (cache) => applyWrite(cache, written))
    },
  })
}

/** Transitions are not optimistic: the row shows the request as pending, then settles or shows the refusal. */
export function useTransitions() {
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
        setIssues(client, (cache) => applyMove(cache, moved))
        set(id, undefined)
        if (moved.warning) toast.warning(moved.warning)
        return true
      } catch (error) {
        const text = error instanceof ApiError ? refusalText(error.body) : String(error)
        set(id, { state: 'refused', to: to.key, toName: to.name, text })
        return false
      }
    },
    [client, set],
  )

  const dismiss = useCallback(
    (id: string) => {
      set(id, undefined)
    },
    [set],
  )

  return { transitions, request, dismiss }
}

export const useIssues = () => useQuery(issuesQuery)
export const useWorkflow = () => useQuery(workflowQuery)
export const useActors = () => useQuery(actorsQuery)
export const useProjects = () => useQuery(projectsQuery)
