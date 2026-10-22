import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import {
  createRootRouteWithContext,
  createRoute,
  createRouter,
  Outlet,
  redirect,
  RouterProvider,
} from '@tanstack/react-router'
import { Toaster } from '@/components/ui/sonner'
import { sessionQuery } from '@/lib/queries'
import { IssueScreen } from '@/screens/issue'
import { IssuesScreen, type ListSearch } from '@/screens/issues'
import { SignInScreen } from '@/screens/sign-in'
import './styles.css'

const queryClient = new QueryClient({ defaultOptions: { queries: { retry: 1, refetchOnWindowFocus: false } } })

const root = createRootRouteWithContext<{ queryClient: QueryClient }>()({
  component: () => (
    <>
      <Outlet />
      <Toaster position="bottom-right" />
    </>
  ),
})

const signIn = createRoute({ getParentRoute: () => root, path: '/sign-in', component: SignInScreen })

const id = (value: unknown) => (typeof value === 'string' && value !== '' ? value : undefined)

const issues = createRoute({
  getParentRoute: () => root,
  path: '/',
  validateSearch: (search: Record<string, unknown>): ListSearch => ({
    layout: search.layout === 'board' ? 'board' : undefined,
    project: id(search.project),
    milestone: id(search.milestone),
  }),
  beforeLoad: async ({ context }) => {
    const session = await context.queryClient.query(sessionQuery)
    if (!session) throw redirect({ to: '/sign-in' })
    return { session }
  },
  component: function Issues() {
    const { session } = issues.useRouteContext()
    const search = issues.useSearch()
    const navigate = issues.useNavigate()
    return (
      <IssuesScreen
        workspaceId={session.workspace_id}
        me={session.actor.name}
        search={search}
        onSearch={({ layout, project, milestone }) =>
          void navigate({ search: { layout, project, milestone }, replace: true })
        }
      />
    )
  },
})

// The issue page. A browser whose requests carry a standalone agent's bearer token shows it as that agent.
const issue = createRoute({
  getParentRoute: () => root,
  path: '/issues/$id',
  beforeLoad: async ({ context }) => {
    const session = await context.queryClient.query(sessionQuery)
    if (!session) throw redirect({ to: '/sign-in' })
    return { session }
  },
  component: function Issue() {
    const { session } = issue.useRouteContext()
    const { id } = issue.useParams()
    return <IssueScreen key={id} id={id} viewer={session.actor} workspaceId={session.workspace_id} />
  },
})

const router = createRouter({ routeTree: root.addChildren([signIn, issues, issue]), context: { queryClient } })

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
