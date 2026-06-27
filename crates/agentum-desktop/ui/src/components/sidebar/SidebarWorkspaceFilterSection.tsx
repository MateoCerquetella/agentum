import React from 'react'
import { GitBranch, Moon } from 'lucide-react'
import { useAppStore } from '@/store'
import { cn } from '@/lib/utils'
import { FilterToggleRow } from './FilterToggleRow'

const SidebarWorkspaceFilterSection = React.memo(function SidebarWorkspaceFilterSection() {
  const showSleepingWorkspaces = useAppStore((s) => s.showSleepingWorkspaces)
  const setShowSleepingWorkspaces = useAppStore((s) => s.setShowSleepingWorkspaces)
  const hideDefaultBranchWorkspace = useAppStore((s) => s.hideDefaultBranchWorkspace)
  const setHideDefaultBranchWorkspace = useAppStore((s) => s.setHideDefaultBranchWorkspace)

  return (
    <>
      <div className="flex items-center justify-between px-2 py-1">
        <span className="text-[11px] font-semibold text-muted-foreground">Filters</span>
      </div>
      <FilterToggleRow
        icon={<Moon className="size-3.5" />}
        label="Hide sleeping"
        checked={!showSleepingWorkspaces}
        onChange={(hideSleeping) => setShowSleepingWorkspaces(!hideSleeping)}
      />
      <FilterToggleRow
        icon={<GitBranch className="size-3.5" />}
        label="Show default branches"
        checked={!hideDefaultBranchWorkspace}
        onChange={(showDefault) => setHideDefaultBranchWorkspace(!showDefault)}
      />
    </>
  )
})

export default SidebarWorkspaceFilterSection
