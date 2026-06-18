import { HardDrive } from 'lucide-react'
import { Button } from '@/components/ui/button'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle
} from '@/components/ui/dialog'

export function TerminalSessionStateSaveFailureDialog({
  open,
  onDismiss
}: {
  open: boolean
  onDismiss: () => void
}): React.JSX.Element {
  return (
    <Dialog
      open={open}
      onOpenChange={(isOpen) => {
        if (!isOpen) {
          onDismiss()
        }
      }}
    >
      <DialogContent className="sm:max-w-md" showCloseButton={false}>
        <DialogHeader className="gap-3">
          <div className="flex items-center gap-3">
            <div className="flex size-8 shrink-0 items-center justify-center rounded-md border border-border bg-muted/40">
              <HardDrive className="size-4 text-muted-foreground" />
            </div>
            <DialogTitle className="text-base">Disk space is unavailable</DialogTitle>
          </div>
          <DialogDescription className="text-xs leading-5">
            Agentum could not save this terminal session because local storage is full or not writable.
            Free up disk space and try again.
          </DialogDescription>
        </DialogHeader>

        <DialogFooter className="gap-2">
          <Button type="button" size="sm" autoFocus onClick={onDismiss}>
            Dismiss
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
