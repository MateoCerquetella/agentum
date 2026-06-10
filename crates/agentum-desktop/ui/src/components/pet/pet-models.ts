import type { CustomPet } from '../../../../shared/types'
import agentUrl from '../../../resources/agent-sprites.png?url'

type Sprite = NonNullable<CustomPet['sprite']>

// Why: the bundled mascot is the agentum-www "agent" character — a single-row
// sprite sheet of 6 distinct poses (not a frame-by-frame walk cycle). Vite's
// `?url` import hashes the asset at build time so it rides the normal caching
// pipeline.
export const DEFAULT_PET_ID = 'agentum-agent'

export type BundledPetId = typeof DEFAULT_PET_ID

export type BundledPet = {
  id: BundledPetId
  label: string
  url: string
  // Why: sprite-sheet layout so the renderer can crop individual poses out of
  // the sheet. The agent mascot drives pose selection via its roaming sim
  // (see AgentRoamer), not the CSS frame-step path.
  sprite?: Sprite
  // Why: opt this pet into the roaming renderer that ports agentum-www's live
  // agent — walking, hopping, and the slip/fall easter-egg.
  behavior?: 'agent'
}

// Sheet layout: 6 columns × 1 row at 259×234 each (1554×234 total).
// Column order matches agentum-www: walk(0) blink(1) happy(2) jump(3)
// slip(4) fallen(5). Locomotion + pose selection live in agent-roamer.ts.
const AGENT_FRAME_W = 259
const AGENT_FRAME_H = 234
const AGENT_COLUMNS = 6

const AGENT_SPRITE: Sprite = {
  frameWidth: AGENT_FRAME_W,
  frameHeight: AGENT_FRAME_H,
  columns: AGENT_COLUMNS,
  rows: 1,
  sheetWidth: AGENT_FRAME_W * AGENT_COLUMNS,
  sheetHeight: AGENT_FRAME_H,
  fps: 8
}

export const BUNDLED_PETS: readonly BundledPet[] = [
  {
    id: DEFAULT_PET_ID,
    label: 'Agent',
    url: agentUrl,
    sprite: AGENT_SPRITE,
    behavior: 'agent'
  }
] as const

export const BUNDLED_PET: BundledPet = BUNDLED_PETS[0]

export function isBundledPetId(id: string | undefined): boolean {
  return BUNDLED_PETS.some((s) => s.id === id)
}

export function findBundledPet(id: string | undefined): BundledPet | undefined {
  return BUNDLED_PETS.find((s) => s.id === id)
}
