import React, { useCallback } from 'react'
import { useAppStore } from '@/store'
import CreateWorkspaceWizard from '@/components/new-workspace/CreateWorkspaceWizard'
import type { CreateWorkspaceWizardData } from '@/components/new-workspace/create-workspace-wizard-model'

/**
 * Spec 013 F4: the New Workspace composer modal is now a thin host for the
 * single front door — `CreateWorkspaceWizard`. The former phase machine (goal /
 * provision / details → the composer card) is gone: every opinionated open
 * (`startGatedRun`, `linkedWorkItem`, `prefilledName`, `initialRepoId`,
 * `initialBaseBranch`, `initialWorkspaceStatus`, `telemetrySource`) is honored
 * by the wizard via `deriveWizardComposerSeed`, and a gated run started here
 * hits the SAME `start_work` precondition set through the SAME `submitQuick`.
 */
export default function NewWorkspaceComposerModal(): React.JSX.Element | null {
  const visible = useAppStore((s) => s.activeModal === 'new-workspace-composer')
  const modalData = useAppStore((s) => s.modalData as CreateWorkspaceWizardData | undefined)
  const closeModal = useAppStore((s) => s.closeModal)

  // Why: Dialog open-state transitions must be driven by the store, not a
  // mirror useState, so palette/open-modal calls feel instantaneous and the
  // modal doesn't linger with stale data after close.
  const handleOpenChange = useCallback(
    (open: boolean) => {
      if (!open) {
        closeModal()
      }
    },
    [closeModal]
  )

  if (!visible) {
    return null
  }

  return (
    <CreateWorkspaceWizard
      modalData={modalData ?? {}}
      onClose={closeModal}
      onOpenChange={handleOpenChange}
    />
  )
}
