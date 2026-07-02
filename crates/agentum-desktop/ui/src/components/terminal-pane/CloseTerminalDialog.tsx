import { Check } from 'lucide-react'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle
} from '@/components/ui/dialog'
import { Button } from '@/components/ui/button'

export default function CloseTerminalDialog({
  open,
  onCancel,
  onConfirm,
  dontAskAgain,
  onDontAskAgainChange
}: {
  open: boolean
  onCancel: () => void
  onConfirm: () => void
  /** Current state of the "Don't ask again" checkbox. When `onDontAskAgainChange`
   *  is omitted the checkbox is hidden (callers that don't persist a preference). */
  dontAskAgain?: boolean
  onDontAskAgainChange?: (next: boolean) => void
}): React.JSX.Element {
  return (
    <Dialog
      open={open}
      onOpenChange={(isOpen) => {
        if (!isOpen) {
          onCancel()
        }
      }}
    >
      <DialogContent className="max-w-sm" showCloseButton={false}>
        <DialogHeader>
          <DialogTitle className="text-sm">Stop running command?</DialogTitle>
          <DialogDescription className="text-xs">
            Closing this terminal will stop the command running inside it.
          </DialogDescription>
        </DialogHeader>
        {onDontAskAgainChange ? (
          // Mirrors the delete-worktree dialog's opt-out checkbox: a plain button
          // styled as a checkbox so it inherits our focus ring without pulling in
          // a form-control dependency.
          <button
            type="button"
            role="checkbox"
            aria-checked={dontAskAgain ?? false}
            onClick={() => onDontAskAgainChange(!(dontAskAgain ?? false))}
            className="flex items-center gap-2 rounded-sm px-1 py-1 text-xs text-foreground/80 transition-colors hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
          >
            <span
              className={`flex size-4 items-center justify-center rounded-sm border transition-colors ${
                dontAskAgain
                  ? 'border-foreground bg-foreground text-background'
                  : 'border-muted-foreground bg-transparent'
              }`}
            >
              {dontAskAgain ? <Check className="size-3" strokeWidth={3} /> : null}
            </span>
            Don&apos;t ask again for running terminals
          </button>
        ) : null}
        <DialogFooter className="gap-2">
          <Button type="button" variant="outline" size="sm" onClick={onCancel}>
            Cancel
          </Button>
          <Button type="button" variant="destructive" size="sm" autoFocus onClick={onConfirm}>
            Stop and Close
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
