import React from 'react'
import { ChevronLeft, ChevronRight, type LucideIcon } from 'lucide-react'
import { useAppStore } from '@/store'

type DrillInHeaderProps = {
  /** Lucide icon for the current view, shown in the breadcrumb tail. */
  icon?: LucideIcon
  /** Current view title — the breadcrumb tail. */
  title: string
  /** One-line description of what this view is for. */
  description?: string
  /**
   * Where the back button goes. Defaults to Mission Control (home) so every
   * drill-in is one click from home. Pass a custom handler to preserve a
   * page-specific guard (e.g. Settings' unsaved-changes prompt).
   */
  onBack?: () => void
  /** Right-aligned actions (refresh, filters, etc.). */
  actions?: React.ReactNode
}

/**
 * Shared drill-in header for every non-home view (Phase 1 nav shell, #48).
 *
 * Renders a back affordance plus a `Mission Control › {title}` breadcrumb whose
 * root always routes home, so no view can become a dead-end. Pairs with the
 * always-visible left rail: the rail gets you anywhere, this gets you home and
 * tells you where you are.
 */
export function DrillInHeader({
  icon: Icon,
  title,
  description,
  onBack,
  actions
}: DrillInHeaderProps): React.JSX.Element {
  const openActivityPage = useAppStore((s) => s.openActivityPage)
  const goHome = onBack ?? openActivityPage
  return (
    <header className="flex h-11 flex-none items-center gap-1.5 border-b border-border px-3">
      <button
        type="button"
        onClick={goHome}
        aria-label="Back to Mission Control"
        className="inline-flex shrink-0 items-center gap-1.5 rounded-md px-2 py-1 text-[13px] text-muted-foreground transition-colors hover:bg-foreground/5 hover:text-foreground"
      >
        <ChevronLeft className="size-4" />
        Mission Control
      </button>
      <ChevronRight className="size-3.5 shrink-0 text-muted-foreground/40" aria-hidden />
      <span className="flex min-w-0 items-center gap-1.5 font-semibold tracking-tight text-foreground">
        {Icon ? <Icon className="size-4 shrink-0 text-primary" aria-hidden /> : null}
        <span className="truncate text-sm">{title}</span>
      </span>
      {description ? (
        <span className="ml-1 hidden min-w-0 truncate font-mono text-[11px] text-muted-foreground lg:inline">
          {description}
        </span>
      ) : null}
      {actions ? <div className="ml-auto flex shrink-0 items-center gap-1">{actions}</div> : null}
    </header>
  )
}

