import type React from 'react'
import { ChevronDown, Monitor, Server, SquareTerminal } from 'lucide-react'
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
      {/* tmux indicator: sessions on this host run inside tmux. Green when
          available; amber "tmux?" when missing (hook for an install prompt). */}
      {host.tmuxInstalled !== undefined ? (
        <span
          title={
            host.tmuxInstalled
              ? 'Sessions on this host run inside tmux'
              : 'tmux is not installed on this host — sessions may not persist'
          }
          className={cn(
            'ml-auto inline-flex items-center gap-1 rounded-full px-1.5 py-0.5 text-[10px] font-medium',
            host.tmuxInstalled
              ? 'bg-emerald-500/12 text-emerald-600 dark:text-emerald-400'
              : 'bg-amber-500/15 text-amber-600 dark:text-amber-400'
          )}
        >
          <SquareTerminal className="size-3 shrink-0" />
          {host.tmuxInstalled ? 'tmux' : 'tmux?'}
        </span>
      ) : null}
      <span
        className={cn(
          'inline-flex items-center gap-1 rounded-full bg-sidebar-accent px-1.5 py-0.5 text-[11px] text-muted-foreground',
          host.tmuxInstalled === undefined && 'ml-auto'
        )}
      >
        <span className={cn('size-1.5 rounded-full', STATUS_DOT[status])} />
        {count}
      </span>
    </div>
  )
}
