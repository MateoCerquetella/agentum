/* eslint-disable max-lines -- Why: the board drawer owns shared board state, drag/drop, and settings callbacks that need one coordinated surface. */
import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { toast } from 'sonner'
import { useAppStore } from '@/store'
import { useAllWorktrees, useRepoMap } from '@/store/selectors'
import { Sheet, SheetContent } from '@/components/ui/sheet'
import { PROJECT_INTEGRATIONS_SECTION_ID } from '@/components/settings/ProjectIntegrationsSection'
import { linearGetIssue } from '@/runtime/runtime-linear-client'
import {
  worktreesReconcileGithubStatus,
  worktreesReconcileLinearStatus,
  worktreesTransitionTracker
} from '@/runtime/server-worktree-client'
import type { TrackerPhaseWire } from '@/lib/tracker-phase'
import WorkspaceKanbanAreaSelectionOverlay from './WorkspaceKanbanAreaSelectionOverlay'
import WorkspaceKanbanDrawerHeader from './WorkspaceKanbanDrawerHeader'
import WorkspaceKanbanLaneGrid from './WorkspaceKanbanLaneGrid'
import WorkspaceKanbanPinDropTarget from './WorkspaceKanbanPinDropTarget'
import { hasWorkspaceDragData, readWorkspaceDragDataIds } from './workspace-status'
import { useWorkspaceStatusDocumentDrop } from './use-workspace-status-drop'
import { useWorkspaceKanbanAreaSelection } from './use-workspace-kanban-area-selection'
import { useWorkspaceKanbanCardPointerDrag } from './use-workspace-kanban-card-pointer-drag'
import { useWorkspaceKanbanColumnResize } from './use-workspace-kanban-column-resize'
import { useWorkspaceKanbanSelection } from './use-workspace-kanban-selection'
import { useWorkspaceKanbanShiftWheelScroll } from './use-workspace-kanban-shift-wheel-scroll'
import {
  isWorkspaceBoardKeepOpenTarget,
  useWorkspaceKanbanOutsideDismiss
} from './use-workspace-kanban-outside-dismiss'
import { useVisibleWorkspaceKanbanWorktreeIds } from './use-visible-workspace-kanban-worktree-ids'
import { resolveIssueProjectStatusForBoard } from './IssueProjectStatusChip'
import {
  WORKSPACE_KANBAN_TRACKER_LANES,
  getWorkspaceKanbanTrackerLane,
  groupWorkspaceKanbanWorktrees
} from './workspace-kanban-worktree-groups'
import {
  commitWorkspaceKanbanTrackerMove,
  hasWorkspaceKanbanTrackerLink,
  isWorkspaceKanbanLifecycleLane,
  refreshWorkspaceKanbanTrackerPhases
} from './workspace-kanban-tracker-board'
import {
  buildManualOrderUpdatesForGroupDrop,
  shouldWriteManualOrderForGroupDrop,
  type WorktreeDragGroup
} from './worktree-manual-order'
import type { WorkspaceStatus, Worktree } from '../../../../shared/types'
import {
  WORKSPACE_BOARD_COLUMN_GAP,
  fitWorkspaceBoardColumnWidth
} from '../../../../shared/workspace-statuses'

type WorkspaceKanbanDrawerProps = {
  open: boolean
  preserveOpenForMenu: boolean
  onOpenChange: (open: boolean) => void
  onMenuOpenChange: (open: boolean) => void
}

export default function WorkspaceKanbanDrawer({
  open,
  preserveOpenForMenu,
  onOpenChange,
  onMenuOpenChange
}: WorkspaceKanbanDrawerProps): React.JSX.Element {
  const allWorktrees = useAllWorktrees()
  const repoMap = useRepoMap()
  const activeWorktreeId = useAppStore((s) => s.activeWorktreeId)
  const updateWorktreeMeta = useAppStore((s) => s.updateWorktreeMeta)
  const updateWorktreesMeta = useAppStore((s) => s.updateWorktreesMeta)
  const settings = useAppStore((s) => s.settings)
  const workspaceBoardColumnLayout = useAppStore((s) => s.workspaceBoardColumnLayout)
  const setWorkspaceBoardColumnLayout = useAppStore((s) => s.setWorkspaceBoardColumnLayout)
  const workspaceBoardColumnWidth = useAppStore((s) => s.workspaceBoardColumnWidth)
  const setWorkspaceBoardColumnWidth = useAppStore((s) => s.setWorkspaceBoardColumnWidth)
  const sortBy = useAppStore((s) => s.sortBy)
  const setSortBy = useAppStore((s) => s.setSortBy)
  const sidebarOpen = useAppStore((s) => s.sidebarOpen)
  const sidebarWidth = useAppStore((s) => s.sidebarWidth)
  const boardRef = useRef<HTMLDivElement>(null)
  const laneScrollerRef = useRef<HTMLDivElement>(null)
  const areaSelectionOverlayRef = useRef<HTMLDivElement>(null)
  const [dragOverStatus, setDragOverStatus] = useState<WorkspaceStatus | null>(null)
  const [pinDragOver, setPinDragOver] = useState(false)
  const [laneScrollerWidth, setLaneScrollerWidth] = useState(0)
  const [confirmedPhases, setConfirmedPhases] = useState<ReadonlyMap<string, TrackerPhaseWire>>(
    () => new Map()
  )
  const refreshGenerationRef = useRef(0)
  const visibleWorktreeIdSet = useVisibleWorkspaceKanbanWorktreeIds({
    allWorktrees,
    repoMap
  })
  const worktreesByStatus = useMemo(() => {
    return groupWorkspaceKanbanWorktrees({
      worktrees: allWorktrees,
      visibleWorktreeIds: visibleWorktreeIdSet,
      confirmedPhases,
      sortBy
    })
  }, [allWorktrees, confirmedPhases, sortBy, visibleWorktreeIdSet])
  const worktreeById = useMemo(
    () => new Map(allWorktrees.map((worktree) => [worktree.id, worktree])),
    [allWorktrees]
  )
  const boardWorktrees = useMemo(
    () =>
      WORKSPACE_KANBAN_TRACKER_LANES.flatMap((status) => worktreesByStatus.get(status.id) ?? []),
    [worktreesByStatus]
  )
  const boardDragGroups = useMemo<WorktreeDragGroup[]>(
    () =>
      WORKSPACE_KANBAN_TRACKER_LANES.map((status) => ({
        key: status.id,
        worktreeIds: (worktreesByStatus.get(status.id) ?? []).map((worktree) => worktree.id)
      })),
    [worktreesByStatus]
  )
  const {
    selectedWorktreeIds,
    selectedWorktrees,
    selectionAnchorId,
    updateSelectionForGesture,
    updateSelectionForArea,
    clearSelection,
    selectForContextMenu
  } = useWorkspaceKanbanSelection(open, boardWorktrees)
  const { handleAreaSelectionPointerDown } = useWorkspaceKanbanAreaSelection({
    open,
    boardRef,
    overlayRef: areaSelectionOverlayRef,
    selectedWorktreeIds,
    selectionAnchorId,
    updateSelectionForArea
  })
  const { columnWidth, isResizingColumn, onColumnResizeStart, onColumnResizeKeyDown } =
    useWorkspaceKanbanColumnResize(workspaceBoardColumnWidth, setWorkspaceBoardColumnWidth)
  const laneScrollerResizeRef = useRef<ResizeObserver | null>(null)
  const attachLaneScroller = useCallback((node: HTMLDivElement | null) => {
    laneScrollerRef.current = node
    laneScrollerResizeRef.current?.disconnect()
    laneScrollerResizeRef.current = null
    if (!node) {
      return
    }
    setLaneScrollerWidth(node.clientWidth)
    if (typeof ResizeObserver === 'undefined') {
      return
    }
    const observer = new ResizeObserver(() => setLaneScrollerWidth(node.clientWidth))
    observer.observe(node)
    laneScrollerResizeRef.current = observer
  }, [])
  const renderColumnWidth = useMemo(
    () =>
      workspaceBoardColumnLayout === 'fit'
        ? fitWorkspaceBoardColumnWidth({
            containerWidth: laneScrollerWidth,
            columnCount: WORKSPACE_KANBAN_TRACKER_LANES.length,
            capWidth: columnWidth
          })
        : columnWidth,
    [columnWidth, laneScrollerWidth, workspaceBoardColumnLayout]
  )
  const visibleLinkedWorktrees = useMemo(
    () =>
      allWorktrees.filter(
        (worktree) =>
          visibleWorktreeIdSet.has(worktree.id) && hasWorkspaceKanbanTrackerLink(worktree)
      ),
    [allWorktrees, visibleWorktreeIdSet]
  )
  const resolveProviderPhase = useCallback(
    async (worktree: Worktree): Promise<TrackerPhaseWire | null> => {
      if (worktree.trackerProvider === 'github') {
        const result = await resolveIssueProjectStatusForBoard({
          issueUrl: worktree.trackerUrl!,
          workdir: worktree.path,
          repoId: worktree.repoId
        })
        if (result.warning) {
          throw new Error(result.warning)
        }
        if (!result.statusOptionId) {
          return null
        }
        const reconciled = await worktreesReconcileGithubStatus(worktree.id, result.statusOptionId)
        return reconciled.phase
      }

      const issueId = worktree.linkedLinearIssue?.trim() || worktree.trackerUrl?.trim()
      if (!issueId) {
        return null
      }
      const issue = await linearGetIssue(settings, issueId)
      if (!issue) {
        throw new Error(`Linear issue not found: ${issueId}`)
      }
      const reconciled = await worktreesReconcileLinearStatus(worktree.id, issue.state.name)
      return reconciled.phase
    },
    [settings]
  )

  useEffect(() => {
    if (!open) {
      refreshGenerationRef.current += 1
      return
    }
    const refresh = (): void => {
      const generation = ++refreshGenerationRef.current
      void refreshWorkspaceKanbanTrackerPhases({
        worktrees: visibleLinkedWorktrees,
        resolvePhase: resolveProviderPhase,
        isCurrent: () => generation === refreshGenerationRef.current
      }).then((next) => {
        if (!next) {
          return
        }
        setConfirmedPhases((current) => {
          const merged = new Map(current)
          for (const [worktreeId, phase] of next) {
            merged.set(worktreeId, phase)
          }
          return merged
        })
      })
    }
    const refreshWhenVisible = (): void => {
      if (document.visibilityState === 'visible') {
        refresh()
      }
    }
    refresh()
    window.addEventListener('focus', refresh)
    document.addEventListener('visibilitychange', refreshWhenVisible)
    return () => {
      refreshGenerationRef.current += 1
      window.removeEventListener('focus', refresh)
      document.removeEventListener('visibilitychange', refreshWhenVisible)
    }
  }, [open, resolveProviderPhase, visibleLinkedWorktrees])

  const getSourceStatusKeys = useCallback(
    (worktreeIds: readonly string[]): WorkspaceStatus[] =>
      worktreeIds.flatMap((worktreeId) => {
        const worktree = worktreeById.get(worktreeId)
        const lane = worktree ? getWorkspaceKanbanTrackerLane(worktree, confirmedPhases) : null
        return lane ? [lane] : []
      }),
    [confirmedPhases, worktreeById]
  )
  const shouldWriteDropManualOrder = useCallback(
    (worktreeIds: readonly string[], status: WorkspaceStatus): boolean =>
      shouldWriteManualOrderForGroupDrop({
        sortBy,
        sourceGroupKeys: getSourceStatusKeys(worktreeIds),
        targetGroupKey: status
      }),
    [getSourceStatusKeys, sortBy]
  )
  const commitManualOrderForDrop = useCallback(
    (args: {
      worktreeIds: readonly string[]
      status: WorkspaceStatus
      dropIndex: number
      writeManualOrder: boolean
    }) => {
      if (!args.writeManualOrder) {
        return
      }
      const rankByWorktreeId = (() => {
        const ranks = new Map<string, number>()
        for (const group of boardDragGroups) {
          for (const worktreeId of group.worktreeIds) {
            const worktree = worktreeById.get(worktreeId)
            if (worktree) {
              ranks.set(worktreeId, worktree.manualOrder ?? worktree.sortOrder)
            }
          }
        }
        return ranks
      })()
      const order = buildManualOrderUpdatesForGroupDrop({
        groups: boardDragGroups,
        targetGroupKey: args.status,
        draggedIds: args.worktreeIds,
        dropIndex: args.dropIndex,
        now: Date.now(),
        rankByWorktreeId
      })
      if (!order.changed || order.updates.size === 0) {
        return
      }
      setSortBy('manual')
      useAppStore.getState().recordFeatureInteraction('workspace-board-actions')
      void updateWorktreesMeta(order.updates)
    },
    [boardDragGroups, setSortBy, updateWorktreesMeta, worktreeById]
  )
  const openTrackerSettings = useCallback((worktree: Worktree) => {
    const store = useAppStore.getState()
    store.openSettingsTarget({
      pane: 'repo',
      repoId: worktree.repoId,
      sectionId: PROJECT_INTEGRATIONS_SECTION_ID
    })
    store.openSettingsPage()
  }, [])
  const dropWorktreesInStatus = useCallback(
    (args: {
      worktreeIds: readonly string[]
      status: WorkspaceStatus
      dropIndex: number
      writeManualOrder?: boolean
    }) => {
      const sourceStatusKeys = getSourceStatusKeys(args.worktreeIds)
      if (sourceStatusKeys.length !== args.worktreeIds.length) {
        return
      }
      const isSameLane = sourceStatusKeys.every((sourceStatus) => sourceStatus === args.status)
      const writeManualOrder =
        args.writeManualOrder ?? shouldWriteDropManualOrder(args.worktreeIds, args.status)
      if (isSameLane) {
        commitManualOrderForDrop({ ...args, writeManualOrder })
        return
      }
      if (args.worktreeIds.length !== 1) {
        toast.error('Move one workspace at a time', {
          description: 'Tracker lifecycle moves do not support multiple selected cards.'
        })
        return
      }
      const worktree = worktreeById.get(args.worktreeIds[0]!)
      if (!worktree || !hasWorkspaceKanbanTrackerLink(worktree)) {
        if (worktree) {
          toast.error('Link a tracker before moving this workspace', {
            description: 'Choose GitHub or Linear in Project Integrations.',
            action: {
              label: 'Link tracker',
              onClick: () => openTrackerSettings(worktree)
            }
          })
        }
        return
      }
      if (!isWorkspaceKanbanLifecycleLane(args.status)) {
        toast.error('The Unlinked lane is assigned automatically', {
          description: 'Remove or change the tracker link from Project Integrations.'
        })
        return
      }

      void commitWorkspaceKanbanTrackerMove({
        worktreeId: worktree.id,
        targetPhase: args.status,
        transition: worktreesTransitionTracker,
        commitPhase: (phase) => {
          setConfirmedPhases((current) => {
            const next = new Map(current)
            next.set(worktree.id, phase)
            return next
          })
        },
        commitManualOrder: () =>
          commitManualOrderForDrop({
            ...args,
            writeManualOrder
          })
      }).then(
        () => {
          useAppStore.getState().recordFeatureInteraction('workspace-board-actions')
        },
        (error) => {
          toast.error('Could not move workspace in its tracker', {
            description: error instanceof Error ? error.message : String(error)
          })
        }
      )
    },
    [
      commitManualOrderForDrop,
      getSourceStatusKeys,
      openTrackerSettings,
      shouldWriteDropManualOrder,
      worktreeById
    ]
  )
  const pinWorktree = useCallback(
    (worktreeId: string) => {
      const current = worktreeById.get(worktreeId)
      if (!current || current.isPinned) {
        return
      }
      void updateWorktreeMeta(worktreeId, { isPinned: true })
    },
    [updateWorktreeMeta, worktreeById]
  )

  const pinWorktrees = useCallback(
    (worktreeIds: readonly string[]) => {
      const updates = new Map<string, { isPinned: true }>()
      for (const worktreeId of worktreeIds) {
        const current = worktreeById.get(worktreeId)
        if (!current || current.isPinned) {
          continue
        }
        updates.set(worktreeId, { isPinned: true })
      }
      if (updates.size > 0) {
        useAppStore.getState().recordFeatureInteraction('workspace-board-actions')
        void updateWorktreesMeta(updates)
      }
    },
    [updateWorktreesMeta, worktreeById]
  )
  const { isPointerDragActiveRef, onCardPointerDownCapture } = useWorkspaceKanbanCardPointerDrag({
    open,
    boardRef,
    selectedWorktreeIds,
    selectedWorktrees,
    onDropWorktreesInStatus: dropWorktreesInStatus,
    onPinWorktrees: pinWorktrees,
    onDragTargetChange: setDragOverStatus,
    onShouldShowDropIndicator: shouldWriteDropManualOrder,
    onPinDragTargetChange: setPinDragOver
  })
  const handleDragOver = useCallback((event: React.DragEvent, status: WorkspaceStatus) => {
    if (!hasWorkspaceDragData(event.dataTransfer)) {
      return
    }
    event.preventDefault()
    event.dataTransfer.dropEffect = 'move'
    setDragOverStatus(status)
  }, [])

  const handleDragLeave = useCallback((event: React.DragEvent) => {
    const relatedTarget = event.relatedTarget
    if (relatedTarget instanceof Node && event.currentTarget.contains(relatedTarget)) {
      return
    }
    setDragOverStatus(null)
  }, [])

  const handlePinDragOver = useCallback((event: React.DragEvent) => {
    if (!hasWorkspaceDragData(event.dataTransfer)) {
      return
    }
    event.preventDefault()
    event.dataTransfer.dropEffect = 'move'
    setPinDragOver(true)
  }, [])

  const handlePinDragLeave = useCallback((event: React.DragEvent) => {
    const relatedTarget = event.relatedTarget
    if (relatedTarget instanceof Node && event.currentTarget.contains(relatedTarget)) {
      return
    }
    setPinDragOver(false)
  }, [])

  const handleDragFinish = useCallback(() => {
    setDragOverStatus(null)
    setPinDragOver(false)
  }, [])

  const dropWorktreesAtEndOfStatus = useCallback(
    (worktreeIds: readonly string[], status: WorkspaceStatus) => {
      dropWorktreesInStatus({
        worktreeIds,
        status,
        dropIndex: worktreesByStatus.get(status)?.length ?? 0,
        writeManualOrder: sortBy === 'manual'
      })
    },
    [dropWorktreesInStatus, sortBy, worktreesByStatus]
  )

  const moveWorktreeToStatus = useCallback(
    (worktreeId: string, status: WorkspaceStatus) => {
      dropWorktreesAtEndOfStatus([worktreeId], status)
    },
    [dropWorktreesAtEndOfStatus]
  )

  const handleDrop = useCallback(
    (event: React.DragEvent, status: WorkspaceStatus) => {
      const worktreeIds = readWorkspaceDragDataIds(event.dataTransfer)
      if (worktreeIds.length === 0) {
        return
      }
      event.preventDefault()
      setDragOverStatus(null)
      dropWorktreesAtEndOfStatus(worktreeIds, status)
    },
    [dropWorktreesAtEndOfStatus]
  )

  const handleWorktreeActivate = useCallback(() => {
    onOpenChange(false)
  }, [onOpenChange])
  const handleHeaderClose = useCallback(() => {
    // Why: generic Radix close requests stay ignored so sidebar drag/outside
    // dismiss rules remain explicit; the header X is a board-owned close path.
    onOpenChange(false)
  }, [onOpenChange])
  const handleSheetOpenChange = useCallback(
    (nextOpen: boolean) => {
      // Why: Radix treats any outside pointer release as a dismiss request.
      // The board has custom right-side/sidebar rules, so only those paths close it.
      if (nextOpen) {
        onOpenChange(true)
      }
    },
    [onOpenChange]
  )

  useWorkspaceStatusDocumentDrop(
    boardRef,
    moveWorktreeToStatus,
    pinWorktree,
    handleDragFinish,
    open,
    {
      onMoveWorktreesToStatus: dropWorktreesAtEndOfStatus,
      onPinWorktrees: pinWorktrees
    }
  )

  useWorkspaceKanbanShiftWheelScroll(boardRef, laneScrollerRef, open, isPointerDragActiveRef)
  useWorkspaceKanbanOutsideDismiss({
    open,
    boardRef,
    preserveOpenForMenu,
    onOpenChange
  })

  useEffect(() => {
    if (!open || selectedWorktreeIds.size === 0) {
      return
    }

    const clearSelectionOutsideBoard = (event: PointerEvent): void => {
      const content = boardRef.current?.closest<HTMLElement>('[data-slot="sheet-content"]')
      const target = event.target
      if (target instanceof Node && content?.contains(target)) {
        return
      }
      if (isWorkspaceBoardKeepOpenTarget(target)) {
        return
      }
      clearSelection()
    }

    // Why: clicks in the sidebar are outside the companion board but do not
    // close it; they still need to behave like "click off" for board selection.
    document.addEventListener('pointerdown', clearSelectionOutsideBoard, true)
    return () => document.removeEventListener('pointerdown', clearSelectionOutsideBoard, true)
  }, [clearSelection, open, selectedWorktreeIds.size])

  const drawerLeft = sidebarOpen ? sidebarWidth : 0
  const drawerLeftCss = sidebarOpen
    ? `var(--workspace-sidebar-live-width, ${sidebarWidth}px)`
    : '0px'
  const fitPanelBoardWidthCss = `min(calc(100vw - ${drawerLeftCss}), 1294px)`
  // Why: full-width mode keeps user-sized columns and expands the companion
  // board over adjacent panes; fit mode preserves the older fixed panel.
  const boardContentWidth =
    WORKSPACE_KANBAN_TRACKER_LANES.length * columnWidth +
    Math.max(0, WORKSPACE_KANBAN_TRACKER_LANES.length - 1) * WORKSPACE_BOARD_COLUMN_GAP +
    24
  const fullBoardWidthCss = `min(calc(100vw - ${drawerLeftCss}), ${Math.max(
    boardContentWidth,
    1294
  )}px)`
  const boardWidthCss =
    workspaceBoardColumnLayout === 'fit' ? fitPanelBoardWidthCss : fullBoardWidthCss

  return (
    <Sheet open={open} onOpenChange={handleSheetOpenChange} modal={false}>
      <SheetContent
        side="left"
        showCloseButton={false}
        className="workspace-kanban-sheet-content bg-sidebar p-0 sm:max-w-none"
        overlayStyle={{ top: 36, left: drawerLeftCss, pointerEvents: 'none' }}
        style={
          {
            // Why: the board is a companion to the workspace sidebar, so it
            // expands from the sidebar edge instead of covering the sidebar.
            left: drawerLeftCss,
            top: 36,
            height: 'calc(100% - 36px)',
            width: boardWidthCss
          } as React.CSSProperties
        }
        onOpenAutoFocus={(event) => {
          // Why: Radix focuses the first toolbar button on open, which opens
          // its tooltip without hover and makes the drawer feel noisy.
          event.preventDefault()
        }}
        onPointerDownOutside={(event) => {
          const originalEvent = event.detail.originalEvent
          const target = originalEvent.target
          if (preserveOpenForMenu) {
            event.preventDefault()
            return
          }
          if (isWorkspaceBoardKeepOpenTarget(target)) {
            event.preventDefault()
            return
          }
          const liveDrawerLeft =
            boardRef.current
              ?.closest<HTMLElement>('[data-slot="sheet-content"]')
              ?.getBoundingClientRect().left ?? drawerLeft
          const pointerX =
            'clientX' in originalEvent && typeof originalEvent.clientX === 'number'
              ? originalEvent.clientX
              : null
          if (pointerX !== null && pointerX < liveDrawerLeft) {
            event.preventDefault()
          }
        }}
        onInteractOutside={(event) => {
          const originalEvent = event.detail.originalEvent
          const target = originalEvent.target
          if (preserveOpenForMenu) {
            // Why: the first outside click should close a board dropdown, not
            // also dismiss the board that owns the dropdown.
            event.preventDefault()
            return
          }
          if (isWorkspaceBoardKeepOpenTarget(target)) {
            event.preventDefault()
            return
          }
          const liveDrawerLeft =
            boardRef.current
              ?.closest<HTMLElement>('[data-slot="sheet-content"]')
              ?.getBoundingClientRect().left ?? drawerLeft
          const pointerX =
            'clientX' in originalEvent && typeof originalEvent.clientX === 'number'
              ? originalEvent.clientX
              : null
          if (pointerX !== null && pointerX < liveDrawerLeft) {
            // Why: keep the workspace sidebar interactive while the companion board stays open.
            event.preventDefault()
          }
        }}
      >
        <WorkspaceKanbanDrawerHeader
          selectedCount={selectedWorktrees.length}
          columnLayout={workspaceBoardColumnLayout}
          onColumnLayoutChange={(layout) => {
            useAppStore.getState().recordFeatureInteraction('workspace-board-actions')
            setWorkspaceBoardColumnLayout(layout)
          }}
          onFilterMenuOpenChange={onMenuOpenChange}
          onClose={handleHeaderClose}
        />
        <div
          ref={boardRef}
          className="relative flex min-h-0 flex-1 flex-col overflow-hidden p-3"
          data-workspace-board-selection-surface=""
          onPointerDownCapture={onCardPointerDownCapture}
          onPointerDown={handleAreaSelectionPointerDown}
        >
          <WorkspaceKanbanAreaSelectionOverlay ref={areaSelectionOverlayRef} />
          <WorkspaceKanbanPinDropTarget
            isDragOver={pinDragOver}
            onDragOver={handlePinDragOver}
            onDragLeave={handlePinDragLeave}
          />
          <div
            ref={attachLaneScroller}
            className="min-h-0 flex-1 overflow-x-auto overflow-y-hidden scrollbar-sleek"
          >
            <WorkspaceKanbanLaneGrid
              statuses={WORKSPACE_KANBAN_TRACKER_LANES}
              worktreesByStatus={worktreesByStatus}
              repoMap={repoMap}
              activeWorktreeId={activeWorktreeId}
              columnWidth={columnWidth}
              renderColumnWidth={renderColumnWidth}
              isResizingColumn={isResizingColumn}
              dragOverStatus={dragOverStatus}
              selectedWorktreeIds={selectedWorktreeIds}
              selectedWorktrees={selectedWorktrees}
              onDragOver={handleDragOver}
              onDragLeave={handleDragLeave}
              onDrop={handleDrop}
              onActivate={handleWorktreeActivate}
              onSelectionGesture={updateSelectionForGesture}
              onContextMenuSelect={selectForContextMenu}
              onColumnResizeStart={onColumnResizeStart}
              onColumnResizeKeyDown={onColumnResizeKeyDown}
            />
          </div>
        </div>
      </SheetContent>
    </Sheet>
  )
}
