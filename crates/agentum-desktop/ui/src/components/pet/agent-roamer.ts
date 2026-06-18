// Port of agentum-www's roaming "agent" — a sprite that walks to random
// targets along the floor, idles, blinks, hops when tapped, and occasionally
// trips (slip → fall → recover). The simulation is a pure reducer so the
// component stays a thin renderer and the physics are unit-testable.
//
// Sheet column order matches agentum-www: walk(0) blink(1) happy(2) jump(3)
// slip(4) fallen(5). Note the website uses HAPPY as the neutral face and
// alternates WALK↔HAPPY for the walk cycle.
export const ROAMER_POSE = {
  walk: 0,
  blink: 1,
  happy: 2,
  jump: 3,
  slip: 4,
  fallen: 5
} as const

export type RoamerPose = (typeof ROAMER_POSE)[keyof typeof ROAMER_POSE]

export type Rng = () => number

/** Live bounds the simulation runs inside, recomputed each frame from the
 *  viewport + pet size so resizing keeps the pet on-screen. `groundY` is the
 *  pet box's top-left y when it stands on the floor; `jz` lifts it above that. */
export type RoamerEnv = {
  size: number
  groundY: number
  minX: number
  maxX: number
}

export type RoamerState = {
  x: number // top-left x of the size×size box
  vx: number
  dir: 1 | -1
  targetX: number
  idleT: number // seconds left standing still
  jz: number // vertical offset, <= 0 means airborne
  jv: number // vertical velocity
  sq: number // landing squash, decays to 0
  emote: 'none' | 'happy'
  emoteT: number
  blinkT: number
  blinking: boolean
  step: 0 | 1
  stepT: number
  moving: boolean
  animPhase: number // accumulates time; drives the idle breathing bob
  bobY: number // small render bob (added to jz at draw time)
  state: 'ok' | 'slip' | 'fallen' | 'getup'
  stateT: number
  nextTripT: number
  dragging: boolean
}

// Durations (s).
const SLIP_SEC = 0.6
const FALLEN_SEC = 2.4
const GETUP_SEC = 0.55
const BLINK_HOLD = 0.12

function rand(min: number, max: number, rng: Rng): number {
  return min + rng() * (max - min)
}

function pickTargetX(env: RoamerEnv, rng: Rng): number {
  return rand(env.minX, env.maxX, rng)
}

// Speeds/forces scale with the pet so a bigger pet still feels right.
function speeds(size: number): { walk: number; grav: number; hop: number; getup: number } {
  return { walk: size * 0.5, grav: size * 16, hop: size * 4, getup: size * 1.6 }
}

export function createRoamerState(env: RoamerEnv, rng: Rng = Math.random): RoamerState {
  return {
    x: pickTargetX(env, rng),
    vx: 0,
    dir: rng() < 0.5 ? -1 : 1,
    targetX: pickTargetX(env, rng),
    idleT: rand(0, 1.2, rng),
    // Why: spawn a little above the floor so the pet drops in on first paint.
    jz: -rand(env.size * 0.25, env.size * 0.55, rng),
    jv: 0,
    sq: 0,
    emote: 'none',
    emoteT: 0,
    blinkT: rand(1, 5, rng),
    blinking: false,
    step: 0,
    stepT: 0,
    moving: false,
    animPhase: rand(0, 6, rng),
    bobY: 0,
    state: 'ok',
    stateT: 0,
    nextTripT: rand(14, 32, rng),
    dragging: false
  }
}

function clampX(s: RoamerState, env: RoamerEnv): void {
  if (s.x < env.minX) {
    s.x = env.minX
    if (s.vx < 0) {
      s.vx = 0
    }
  } else if (s.x > env.maxX) {
    s.x = env.maxX
    if (s.vx > 0) {
      s.vx = 0
    }
  }
}

/** Advance the simulation by `dtSec` (callers clamp dt). Pure given the same
 *  rng draws. While `dragging`, position is owned by the pointer handlers, so
 *  this only decays the landing squash. */
export function advanceRoamer(
  state: RoamerState,
  env: RoamerEnv,
  dtSec: number,
  rng: Rng = Math.random
): RoamerState {
  const s = { ...state }
  const { walk: WALK, grav: GRAV, hop: HOP, getup: GETUP } = speeds(env.size)

  s.animPhase += dtSec
  s.sq *= 0.82

  if (s.dragging) {
    return s
  }

  // Vertical hop physics (gravity), runs in every state so a tripped pet still
  // falls and a tapped pet arcs back down.
  if (s.jz < 0 || s.jv !== 0) {
    s.jv += GRAV * dtSec
    s.jz += s.jv * dtSec
    if (s.jz >= 0) {
      s.jz = 0
      s.jv = 0
      s.sq = 1
    }
  }

  if (s.state !== 'ok') {
    s.stateT += dtSec
    if (s.state === 'slip') {
      s.vx *= 0.88
      s.x += s.vx * dtSec
      clampX(s, env)
      if (s.stateT > SLIP_SEC) {
        s.state = 'fallen'
        s.stateT = 0
        s.vx = 0
      }
    } else if (s.state === 'fallen') {
      s.vx = 0
      if (s.stateT > FALLEN_SEC) {
        s.state = 'getup'
        s.stateT = 0
        s.jv = -GETUP
      }
    } else if (s.state === 'getup') {
      if (s.stateT > GETUP_SEC) {
        s.state = 'ok'
        s.stateT = 0
        s.idleT = rand(0.3, 1, rng)
        s.targetX = pickTargetX(env, rng)
      }
    }
    s.moving = false
    // Subtle sway while down so a fallen pet still looks alive.
    s.bobY = s.state === 'fallen' ? Math.sin(s.animPhase * 4) * env.size * 0.008 : 0
    return s
  }

  // Emote countdown.
  if (s.emoteT > 0) {
    s.emoteT -= dtSec
  } else {
    s.emote = 'none'
  }

  // Seek the target, or idle once we arrive.
  let desiredVx = 0
  if (s.idleT > 0) {
    s.idleT -= dtSec
  } else {
    const dx = s.targetX - s.x
    if (Math.abs(dx) < 6) {
      s.idleT = rand(0.5, 2.2, rng)
      s.targetX = pickTargetX(env, rng)
    } else {
      desiredVx = Math.sign(dx) * WALK
    }
  }

  // Smooth toward desired velocity, integrate, keep inside the floor span.
  const k = Math.min(1, dtSec * 9)
  s.vx += (desiredVx - s.vx) * k
  s.x += s.vx * dtSec
  const hitWall = s.x <= env.minX || s.x >= env.maxX
  clampX(s, env)
  if (hitWall && s.idleT <= 0) {
    s.targetX = pickTargetX(env, rng)
  }

  if (s.vx > WALK * 0.25) {
    s.dir = 1
  } else if (s.vx < -WALK * 0.25) {
    s.dir = -1
  }
  s.moving = Math.abs(s.vx) > WALK * 0.32

  // Trip easter-egg: only when grounded and standing around.
  s.nextTripT -= dtSec
  if (!s.moving && s.idleT > 0 && s.jz === 0 && s.nextTripT <= 0) {
    s.state = 'slip'
    s.stateT = 0
    s.vx = -s.dir * WALK * 1.4
    s.jv = -HOP * 0.45
    s.nextTripT = rand(26, 52, rng)
  }

  // Walk cycle vs blink, plus a small bob.
  if (s.moving) {
    s.stepT += dtSec
    if (s.stepT > 0.16) {
      s.stepT = 0
      s.step = s.step ? 0 : 1
    }
    s.bobY = s.step ? -env.size * 0.03 : 0
  } else {
    s.blinkT -= dtSec
    if (s.blinkT < 0) {
      s.blinking = true
      if (s.blinkT < -BLINK_HOLD) {
        s.blinking = false
        s.blinkT = rand(2.2, 5, rng)
      }
    }
    // Constant breathing bob so a standing pet is never dead-still (matches www).
    s.bobY = Math.sin(s.animPhase * 5.5) * env.size * 0.016
  }

  return s
}

/** Make the pet hop happily (tap) — an upward impulse + happy face. */
export function happyHop(state: RoamerState, size: number): RoamerState {
  const { hop } = speeds(size)
  return { ...state, emote: 'happy', emoteT: 1, jv: -hop, jz: state.jz < 0 ? state.jz : -0.01 }
}

/** Sheet column to draw for the current state. */
export function roamerPose(s: RoamerState): RoamerPose {
  if (s.state === 'slip' || s.state === 'getup') {
    return ROAMER_POSE.slip
  }
  if (s.state === 'fallen') {
    return ROAMER_POSE.fallen
  }
  if (s.jz < -1) {
    return ROAMER_POSE.jump
  }
  if (s.emote === 'happy') {
    return ROAMER_POSE.happy
  }
  if (s.moving) {
    // Walk cycle alternates the walk and (neutral) happy frames.
    return s.step ? ROAMER_POSE.walk : ROAMER_POSE.happy
  }
  return s.blinking ? ROAMER_POSE.blink : ROAMER_POSE.happy
}
