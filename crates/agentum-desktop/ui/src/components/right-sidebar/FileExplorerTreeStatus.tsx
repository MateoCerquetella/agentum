import React from 'react'
import { Loader2, WifiOff } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { isHostUnreachableFsError } from './file-explorer-error'

type FileExplorerTreeStatusProps = {
  isLoading: boolean
  error: string | null
  isEmpty: boolean
  onRetry?: () => void
}

export function FileExplorerTreeStatus({
  isLoading,
  error,
  isEmpty,
  onRetry
}: FileExplorerTreeStatusProps): React.JSX.Element | null {
  if (isLoading) {
    return (
      <div className="flex h-full items-center justify-center text-[11px] text-muted-foreground">
        <Loader2 className="size-4 animate-spin" />
      </div>
    )
  }

  if (error) {
    // Why: an unreachable SSH host is an environment state, not a bug — the raw
    // transport error (URL-encoded path, host UUID, JSON envelope) reads like a
    // crash. Other errors keep their message so path/auth bugs stay diagnosable.
    if (isHostUnreachableFsError(error)) {
      return (
        <div className="flex h-full flex-col items-center justify-center gap-1.5 px-4 text-center">
          <WifiOff className="size-6 text-muted-foreground opacity-50" />
          <div className="text-xs font-medium">Host unreachable</div>
          <div className="text-[11px] text-muted-foreground">
            Could not connect to the SSH host for this workspace. Files will load again once the
            connection is back.
          </div>
          {onRetry && (
            <Button type="button" variant="outline" size="sm" className="mt-1.5" onClick={onRetry}>
              Retry
            </Button>
          )}
        </div>
      )
    }
    return (
      <div className="flex h-full items-center justify-center px-4 text-center text-[11px] text-muted-foreground">
        Could not load files for this workspace: {error}
      </div>
    )
  }

  if (isEmpty) {
    return (
      <div className="flex h-full items-center justify-center px-4 text-center text-[11px] text-muted-foreground">
        No files in this workspace
      </div>
    )
  }

  return null
}
