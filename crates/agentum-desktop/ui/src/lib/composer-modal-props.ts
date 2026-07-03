// Spec 008 F1 #1 (AC 1): the Tasks-page pre-armed hop
// (`openComposerForItem(item, { startGatedRun: true })`) sets `modalData
// .startGatedRun`, which the composer modal must translate into the composer's
// `initialStartGatedRun` prop so the "Start gated run" toggle opens ALREADY
// armed. Extracted from the modal's inline spread into this pure, named mapping
// so that leg is unit-pinned — the armed toggle can never silently fail to arm.

/**
 * Map the modal-open data's `startGatedRun` flag to the composer's
 * `initialStartGatedRun` prop, as a spread-able partial: an absent/false flag
 * yields `{}` so the prop is left at its default (byte-identical to the old
 * inline `? { ... } : {}`); a true flag yields `{ initialStartGatedRun: true }`.
 */
export function initialStartGatedRunProp(
  modalData: { startGatedRun?: boolean } | null | undefined
): { initialStartGatedRun: true } | Record<string, never> {
  return modalData?.startGatedRun ? { initialStartGatedRun: true } : {}
}
