import type React from 'react'
import { ChevronDown, Monitor, Server } from 'lucide-react'
import { cn } from '@/lib/utils'
import type { SidebarHost } from './worktree-list-groups'

const STATUS_DOT: Record<NonNullable<SidebarHost['status']>, string> = {
  reachable: 'bg-emerald-500',
  connecting: 'bg-amber-500',
  down: 'bg-zinc-400',
  unknown: 'bg-zinc-300'
}

export function HostGroupHeader({
  host,
  count,
  collapsed,
  onToggle
}: {
  host: SidebarHost
  count: number
  collapsed: boolean
  onToggle: () => void
}): React.JSX.Element {
  const Icon = host.kind === 'ssh' ? Server : Monitor
  const status = host.status ?? 'unknown'
  return (
    <div
      role="button"
      tabIndex={0}
      onClick={onToggle}
      onKeyDown={(e) => {
        if (e.key === 'Enter' || e.key === ' ') {
          e.preventDefault()
          onToggle()
        }
      }}
      className="group flex h-8 w-full cursor-pointer items-center gap-1.5 px-1 text-left"
    >
      <ChevronDown
        className={cn(
          'size-3.5 shrink-0 text-muted-foreground transition-transform',
          collapsed && '-rotate-90'
        )}
      />
      <Icon className="size-4 shrink-0 text-muted-foreground" />
      <div className="flex min-w-0 flex-1 flex-col leading-tight">
        <span className="truncate text-sm font-semibold text-foreground">{host.label}</span>
        {host.detail ? (
          <span className="truncate text-[11px] text-muted-foreground">{host.detail}</span>
        ) : null}
      </div>
      <span className="ml-auto inline-flex items-center gap-1 rounded-full bg-sidebar-accent px-1.5 py-0.5 text-[11px] text-muted-foreground">
        <span className={cn('size-1.5 rounded-full', STATUS_DOT[status])} />
        {count}
      </span>
    </div>
  )
}
