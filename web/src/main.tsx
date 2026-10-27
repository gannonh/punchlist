import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import { MutationCache, QueryCache, QueryClient, QueryClientProvider } from '@tanstack/react-query'
import {
  createRootRouteWithContext,
  createRoute,
  createRouter,
  Outlet,
  redirect,
  RouterProvider,
} from '@tanstack/react-router'
import { ApiError } from '@/api/client'
import { Toaster } from '@/components/ui/sonner'
import { sessionQuery, workspacesQuery } from '@/lib/queries'
import { issuePrefix, pickWorkspace } from '@/model/workspaces'
import { InboxScreen } from '@/screens/inbox'
import { IssueScreen } from '@/screens/issue'
import { IssuesScreen, type ListSearch } from '@/screens/issues'
import { ProjectsScreen } from '@/screens/projects'
import { PullRequestsScreen } from '@/screens/pull-requests'
import { SignInScreen } from '@/screens/sign-in'
import './styles.css'

const unauthorized = (error: unknown) => error instanceof ApiError && error.status === 401

// A 401 means the session ended: signed out here, elsewhere (`Sessions`) or expired. Any read or write that gets one
// shows the sign-in page, as a reload would.
const toSignIn = (error: unknown) => {
  if (!unauthorized(error) || router.state.location.pathname === '/sign-in') return
  queryClient.clear()
  queryClient.setQueryData(sessionQuery.queryKey, null)
  void router.navigate({ to: '/sign-in' })
}

const queryClient = new QueryClient({
  queryCache: new QueryCache({ onError: toSignIn }),
  mutationCache: new MutationCache({ onError: toSignIn }),
  defaultOptions: {
    queries: { retry: (failures, error) => failures < 1 && !unauthorized(error), refetchOnWindowFocus: false },
  },
})

const root = createRootRouteWithContext<{ queryClient: QueryClient }>()({
  component: () => (
    <>
      <Outlet />
      <Toaster position="bottom-right" />
    </>
  ),
})

const id = (value: unknown) => (typeof value === 'string' && value !== '' ? value : undefined)
const ids = (value: unknown) => {
  const list = [value].flat().filter((item): item is string => typeof item === 'string' && item !== '')
  return list.length > 0 ? list : undefined
}

const signIn = createRoute({
  getParentRoute: () => root,
  path: '/sign-in',
  // `unlinked`: the GitHub login the server found no account for.
  validateSearch: (search: Record<string, unknown>): { unlinked?: string } => ({ unlinked: id(search.unlinked) }),
  component: SignInScreen,
})

// Search values are plain strings, and a list is the key repeated: `?label=idea&label=ui`.
function parseSearch(text: string): Record<string, string | string[]> {
  const search: Record<string, string | string[]> = {}
  for (const [key, value] of new URLSearchParams(text)) {
    const before = search[key]
    search[key] = before === undefined ? value : [before, value].flat()
  }
  return search
}

function stringifySearch(search: Record<string, unknown>): string {
  const params = new URLSearchParams()
  for (const [key, value] of Object.entries(search))
    for (const item of [value].flat()) if (typeof item === 'string') params.append(key, item)
  const text = params.toString()
  return text ? `?${text}` : ''
}

const signedIn = async (queryClient: QueryClient) => {
  const session = await queryClient.query(sessionQuery)
  if (!session) throw redirect({ to: '/sign-in' })
  return session
}

// The workspace `/` opens, by prefix. Storage can be refused (a private window, a full quota): then `/` opens the
// oldest workspace.
const LAST_WORKSPACE = 'punchlist:workspace'
const lastWorkspace = () => {
  try {
    return localStorage.getItem(LAST_WORKSPACE)
  } catch {
    return null
  }
}
const rememberWorkspace = (prefix: string) => {
  try {
    localStorage.setItem(LAST_WORKSPACE, prefix)
  } catch {
    // `/` opens the oldest workspace instead.
  }
}

// `/` and the paths from before workspaces were in the URL open the last workspace used, else the oldest.
const toWorkspace = async (queryClient: QueryClient) => {
  await signedIn(queryClient)
  const prefix = pickWorkspace(await queryClient.query(workspacesQuery), lastWorkspace())
  if (!prefix) throw new Error('this account is in no workspace')
  return prefix
}

const home = createRoute({
  getParentRoute: () => root,
  path: '/',
  beforeLoad: async ({ context, search }) => {
    throw redirect({ to: '/w/$prefix', params: { prefix: await toWorkspace(context.queryClient) }, search })
  },
})

const oldPullRequests = createRoute({
  getParentRoute: () => root,
  path: '/pull-requests',
  beforeLoad: async ({ context }) => {
    throw redirect({ to: '/w/$prefix/pull-requests', params: { prefix: await toWorkspace(context.queryClient) } })
  },
})

const oldInbox = createRoute({
  getParentRoute: () => root,
  path: '/inbox',
  beforeLoad: async ({ context }) => {
    throw redirect({ to: '/w/$prefix/inbox', params: { prefix: await toWorkspace(context.queryClient) } })
  },
})

const oldProjects = createRoute({
  getParentRoute: () => root,
  path: '/projects',
  beforeLoad: async ({ context }) => {
    throw redirect({ to: '/w/$prefix/projects', params: { prefix: await toWorkspace(context.queryClient) } })
  },
})

// A workspace's screens. A prefix the account is not in opens `/`.
const workspace = createRoute({
  getParentRoute: () => root,
  path: '/w/$prefix',
  beforeLoad: async ({ context, params }) => {
    const session = await signedIn(context.queryClient)
    const workspace = (await context.queryClient.query(workspacesQuery)).find(
      (w) => w.issue_prefix === params.prefix,
    )
    if (!workspace) throw redirect({ to: '/' })
    rememberWorkspace(workspace.issue_prefix)
    return { session, workspace }
  },
})

const issues = createRoute({
  getParentRoute: () => workspace,
  path: '/',
  validateSearch: (search: Record<string, unknown>): ListSearch => ({
    layout: search.layout === 'board' ? 'board' : undefined,
    project: id(search.project),
    milestone: id(search.milestone),
    label: ids(search.label),
    status: ids(search.status),
  }),
  component: function Issues() {
    const { session, workspace } = issues.useRouteContext()
    const search = issues.useSearch()
    const navigate = issues.useNavigate()
    return (
      <IssuesScreen
        key={workspace.id}
        workspace={workspace}
        me={session.actor.name}
        search={search}
        onSearch={({ layout, project, milestone, label, status }) =>
          void navigate({ search: { layout, project, milestone, label, status }, replace: true })
        }
      />
    )
  },
})

const pullRequests = createRoute({
  getParentRoute: () => workspace,
  path: '/pull-requests',
  component: function PullRequests() {
    const { session, workspace } = pullRequests.useRouteContext()
    return <PullRequestsScreen key={workspace.id} workspace={workspace} me={session.actor.name} />
  },
})

const inbox = createRoute({
  getParentRoute: () => workspace,
  path: '/inbox',
  component: function Inbox() {
    const { session, workspace } = inbox.useRouteContext()
    return <InboxScreen key={workspace.id} workspace={workspace} me={session.actor.name} />
  },
})

const projects = createRoute({
  getParentRoute: () => workspace,
  path: '/projects',
  component: function Projects() {
    const { session, workspace } = projects.useRouteContext()
    return <ProjectsScreen key={workspace.id} workspace={workspace} me={session.actor.name} />
  },
})

// The issue page, in the issue's workspace. A browser whose requests carry a standalone agent's bearer token shows
// it as that agent, in the agent's workspace, with no workspace menu.
const issue = createRoute({
  getParentRoute: () => root,
  path: '/issues/$id',
  beforeLoad: async ({ context, params }) => {
    const session = await signedIn(context.queryClient)
    const workspaces =
      session.actor.role === 'person' ? await context.queryClient.query(workspacesQuery) : []
    const workspace = workspaces.find((w) => w.issue_prefix === issuePrefix(params.id))
    return { session, workspace }
  },
  component: function Issue() {
    const { session, workspace } = issue.useRouteContext()
    const { id } = issue.useParams()
    return (
      <IssueScreen
        key={id}
        id={id}
        viewer={session.actor}
        workspace={workspace}
        workspaceId={workspace?.id ?? session.workspace_id}
      />
    )
  },
})

const router = createRouter({
  routeTree: root.addChildren([
    signIn,
    home,
    oldPullRequests,
    oldInbox,
    oldProjects,
    workspace.addChildren([issues, pullRequests, inbox, projects]),
    issue,
  ]),
  context: { queryClient },
  parseSearch,
  stringifySearch,
})

declare module '@tanstack/react-router' {
  interface Register {
    router: typeof router
  }
}

const element = document.getElementById('root')
if (!element) throw new Error('index.html has no #root')
createRoot(element).render(
  <StrictMode>
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} />
    </QueryClientProvider>
  </StrictMode>,
)
