import { api } from '@/tauri'
import React, { useState } from 'react'
import {
  CircleHelp,
  FolderPlus,
  RotateCw,
  School,
  Settings
} from 'lucide-react'
import { useAppStore } from '@/store'
import { Button } from '@/components/ui/button'
import { Tooltip, TooltipTrigger, TooltipContent } from '@/components/ui/tooltip'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger
} from '@/components/ui/dropdown-menu'
import { toast } from 'sonner'
import { useMountedRef } from '@/hooks/useMountedRef'
import { showOnboardingFromRenderer } from '../onboarding/show-onboarding-event'
import { ScrollToCurrentWorkspaceToolbarButton } from './ScrollToCurrentWorkspaceToolbarButton'

const SidebarToolbar = React.memo(function SidebarToolbar() {
  const openModal = useAppStore((s) => s.openModal)
  const openSettingsPage = useAppStore((s) => s.openSettingsPage)
  const [helpMenuOpen, setHelpMenuOpen] = useState(false)
  const [showAdminHelpOptions, setShowAdminHelpOptions] = useState(false)
  const [isRestartingAgentum, setIsRestartingAgentum] = useState(false)
  const lastShowOnboardingAtRef = React.useRef(0)
  const mountedRef = useMountedRef()

  const handleShowOnboarding = (): void => {
    const now = Date.now()
    if (now - lastShowOnboardingAtRef.current < 500) {
      return
    }
    lastShowOnboardingAtRef.current = now
    void showOnboardingFromRenderer()
  }

  const handleHelpMenuOpenChange = (open: boolean): void => {
    setHelpMenuOpen(open)
    if (!open) {
      setShowAdminHelpOptions(false)
    }
  }

  const revealAdminHelpOptions = (altKey: boolean): void => {
    // Why: keep restart off the ordinary Help menu; Alt/Option-click is an
    // intentional admin affordance for recovering the app without teaching it
    // as a normal user workflow.
    setShowAdminHelpOptions(altKey)
  }

  const handleRestartAgentum = (): void => {
    if (isRestartingAgentum) {
      return
    }
    setIsRestartingAgentum(true)
    toast.info('Restarting Agentum…')
    void api.app.restart().catch((error) => {
      if (mountedRef.current) {
        setIsRestartingAgentum(false)
        toast.error('Couldn’t restart Agentum.', {
          description: error instanceof Error ? error.message : undefined
        })
      }
    })
  }

  return (
    <div className="mt-auto shrink-0">
      <div className="flex items-center justify-between border-t border-sidebar-border px-2 py-1.5">
        <Tooltip>
          <TooltipTrigger asChild>
            <Button
              variant="ghost"
              size="xs"
              onClick={() => openModal('add-repo')}
              className="gap-1.5 text-muted-foreground"
            >
              <FolderPlus className="size-3.5" />
              <span className="text-[11px]">Add Project</span>
            </Button>
          </TooltipTrigger>
          <TooltipContent side="top" sideOffset={4}>
            Open folder picker to add a project
          </TooltipContent>
        </Tooltip>
        <div className="flex items-center gap-1">
          <ScrollToCurrentWorkspaceToolbarButton />
          <DropdownMenu modal={false} open={helpMenuOpen} onOpenChange={handleHelpMenuOpenChange}>
            <Tooltip>
              <TooltipTrigger asChild>
                <DropdownMenuTrigger asChild>
                  <Button
                    variant="ghost"
                    size="icon-xs"
                    type="button"
                    aria-label="Help"
                    className="text-muted-foreground"
                    onPointerDown={(event) => revealAdminHelpOptions(event.altKey)}
                    onClick={(event) => revealAdminHelpOptions(event.altKey)}
                  >
                    <CircleHelp className="size-3.5" />
                  </Button>
                </DropdownMenuTrigger>
              </TooltipTrigger>
              <TooltipContent side="top" sideOffset={4}>
                Help
              </TooltipContent>
            </Tooltip>
            <DropdownMenuContent side="top" align="start" sideOffset={8} className="w-48">
              <DropdownMenuItem
                className="whitespace-nowrap"
                onClick={handleShowOnboarding}
                onSelect={handleShowOnboarding}
              >
                <School className="size-3.5" />
                Show Onboarding
              </DropdownMenuItem>
              {showAdminHelpOptions ? (
                <>
                  <DropdownMenuSeparator />
                  <DropdownMenuItem onSelect={handleRestartAgentum} disabled={isRestartingAgentum}>
                    <RotateCw className="size-3.5" />
                    Restart Agentum
                  </DropdownMenuItem>
                </>
              ) : null}
            </DropdownMenuContent>
          </DropdownMenu>
          <Tooltip>
            <TooltipTrigger asChild>
              <Button
                variant="ghost"
                size="icon-xs"
                onClick={openSettingsPage}
                className="text-muted-foreground"
              >
                <Settings className="size-3.5" />
              </Button>
            </TooltipTrigger>
            <TooltipContent side="top" sideOffset={4}>
              Settings
            </TooltipContent>
          </Tooltip>
        </div>
      </div>
    </div>
  )
})

export default SidebarToolbar
