import React from 'react'
import { Settings } from 'lucide-react'
import { Button } from '@/components/ui/button'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuLabel,
  DropdownMenuTrigger
} from '@/components/ui/dropdown-menu'
import { ToggleGroup, ToggleGroupItem } from '@/components/ui/toggle-group'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import type { WorkspaceBoardColumnLayout } from '../../../../shared/types'

type WorkspaceKanbanSettingsMenuProps = {
  columnLayout: WorkspaceBoardColumnLayout
  onColumnLayoutChange: (layout: WorkspaceBoardColumnLayout) => void
}

export default function WorkspaceKanbanSettingsMenu({
  columnLayout,
  onColumnLayoutChange
}: WorkspaceKanbanSettingsMenuProps): React.JSX.Element {
  return (
    <DropdownMenu modal={false}>
      <Tooltip>
        <TooltipTrigger asChild>
          <DropdownMenuTrigger asChild>
            <Button
              variant="ghost"
              size="icon-xs"
              aria-label="Workspace board settings"
              className="text-muted-foreground"
            >
              <Settings className="size-3.5" />
            </Button>
          </DropdownMenuTrigger>
        </TooltipTrigger>
        <TooltipContent side="top" sideOffset={4}>
          Board settings
        </TooltipContent>
      </Tooltip>
      <DropdownMenuContent
        align="end"
        sideOffset={8}
        collisionPadding={8}
        className="max-h-[min(80vh,720px)] w-80 overflow-y-auto p-2 scrollbar-sleek"
      >
        <DropdownMenuLabel>Column layout</DropdownMenuLabel>
        <div className="px-2 pt-0.5 pb-2">
          <ToggleGroup
            type="single"
            value={columnLayout}
            onValueChange={(value) => {
              if (value === 'full' || value === 'fit') {
                onColumnLayoutChange(value)
              }
            }}
            variant="outline"
            size="sm"
            className="h-7 w-full justify-stretch"
            aria-label="Workspace board column layout"
          >
            <ToggleGroupItem
              value="full"
              className="h-7 grow basis-0 px-1.5 text-[11px] data-[state=on]:bg-foreground/10 data-[state=on]:font-semibold data-[state=on]:text-foreground"
            >
              Full width
            </ToggleGroupItem>
            <ToggleGroupItem
              value="fit"
              className="h-7 grow basis-0 px-1.5 text-[11px] data-[state=on]:bg-foreground/10 data-[state=on]:font-semibold data-[state=on]:text-foreground"
            >
              Fit panel
            </ToggleGroupItem>
          </ToggleGroup>
        </div>
      </DropdownMenuContent>
    </DropdownMenu>
  )
}
