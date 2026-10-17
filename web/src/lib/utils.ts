import { createCn } from 'cn/config'

// The type scale in src/styles.css. Without it, cn reads text-meta as a color and keeps the component's text-sm.
export const TYPE_SCALE = ['caption', 'meta', 'body', 'lead', 'title'] as const

// tsconfig.json maps the bare `cn` import here, so the shadcn components keep the CLI's `import { cn } from "cn"`.
export const cn = createCn({ extend: { theme: { text: [...TYPE_SCALE] } } })
