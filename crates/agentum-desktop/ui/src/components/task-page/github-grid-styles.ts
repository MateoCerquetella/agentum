import { cn } from '@/lib/utils'

export const GITHUB_TASK_GRID_CLASS =
  'min-w-[790px] grid-cols-[72px_minmax(320px,1fr)_84px_100px_92px_122px]'
export const GITHUB_PR_TASK_GRID_CLASS =
  'min-w-[1020px] grid-cols-[72px_minmax(360px,2fr)_132px_128px_132px_92px_158px]'
export const GITHUB_TASK_ROW_SURFACE_CLASS =
  '[background:color-mix(in_srgb,var(--muted)_50%,var(--background))]'
export const GITHUB_TASK_ROW_HOVER_SURFACE_CLASS =
  'group-hover/github-task-row:[background:color-mix(in_srgb,var(--muted)_70%,var(--background))]'
// Why: the row's px-3 left padding leaves a 12px gap between the scroll-viewport
// edge and the sticky ID column; without a covering ::before, scrolled cell text
// bleeds through that strip. Same trick as the title column for its 8px gap.
export const GITHUB_TASK_STICKY_ID_HEADER_CLASS = cn(
  'sticky left-3 z-30 before:absolute before:-left-3 before:top-0 before:bottom-0 before:w-3 before:bg-inherit',
  GITHUB_TASK_ROW_SURFACE_CLASS
)
export const GITHUB_TASK_STICKY_TITLE_HEADER_CLASS = cn(
  'sticky left-[92px] z-30 border-r border-border/50 before:absolute before:-left-2 before:top-0 before:bottom-0 before:w-2 before:bg-inherit',
  GITHUB_TASK_ROW_SURFACE_CLASS
)
export const GITHUB_TASK_STICKY_ID_CELL_CLASS = cn(
  'sticky left-3 z-20 flex items-center before:absolute before:-left-3 before:top-0 before:bottom-0 before:w-3 before:bg-inherit',
  GITHUB_TASK_ROW_SURFACE_CLASS,
  GITHUB_TASK_ROW_HOVER_SURFACE_CLASS
)
export const GITHUB_TASK_STICKY_TITLE_CELL_CLASS = cn(
  'sticky left-[92px] z-20 min-w-0 border-r border-border/50 pr-2 before:absolute before:-left-2 before:top-0 before:bottom-0 before:w-2 before:bg-inherit',
  GITHUB_TASK_ROW_SURFACE_CLASS,
  GITHUB_TASK_ROW_HOVER_SURFACE_CLASS
)
