import React from 'react'
import { cn } from '@/lib/utils'
import type { GitHubRepoReference } from './comment-github-references'

// Re-export the pure (react-markdown-free) GitHub autolink plugin so existing
// importers keep working without pulling the heavy renderer into their graph.
export { remarkGitHubReferences } from './comment-github-references'
export type { GitHubRepoReference } from './comment-github-references'

export type CommentMarkdownProps = React.ComponentPropsWithoutRef<'div'> & {
  content: string
  variant?: 'compact' | 'document'
  githubRepo?: GitHubRepoReference | null
}

// Why this wrapper is lazy: the markdown engine (react-markdown + remark/rehype
// plugins + micromark/mdast/hast) is ~150 KB and was being baked into the eager
// startup chunk via the always-mounted Sidebar (worktree cards, dashboard rows)
// and right-sidebar. Deferring it here removes that weight from first paint for
// EVERY call site at once — important on weak CPUs where parsing the entry chunk
// dominates launch time. React.lazy caches the resolved module, so the plaintext
// fallback below is shown at most once per session (the first comment rendered),
// after which all comments render markdown synchronously.
const CommentMarkdownImpl = React.lazy(() => import('./CommentMarkdownImpl'))

// Plaintext fallback while the markdown chunk loads. Preserves newlines so the
// transient view matches the pre-markdown plain-text rendering and keeps card
// sizing stable. Local Tauri assets load near-instantly, so this is typically a
// single frame.
const CommentMarkdownFallback = React.forwardRef<HTMLDivElement, CommentMarkdownProps>(
  function CommentMarkdownFallback({ content, className, variant: _variant, githubRepo: _githubRepo, ...rest }, ref) {
    return (
      <div
        ref={ref}
        className={cn('min-w-0 max-w-full whitespace-pre-wrap [overflow-wrap:anywhere]', className)}
        {...rest}
      >
        {content}
      </div>
    )
  }
)

const CommentMarkdown = React.forwardRef<HTMLDivElement, CommentMarkdownProps>(
  function CommentMarkdown(props, ref) {
    return (
      <React.Suspense fallback={<CommentMarkdownFallback {...props} ref={ref} />}>
        <CommentMarkdownImpl {...props} ref={ref} />
      </React.Suspense>
    )
  }
)

export default CommentMarkdown
