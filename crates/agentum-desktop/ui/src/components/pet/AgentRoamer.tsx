import { useCallback, useEffect, useRef } from 'react'
import type { CustomPet } from '../../../../shared/types'
import { useAppStore } from '../../store'
import { useDocumentVisible } from './useDocumentVisible'
import {
  advanceRoamer,
  createRoamerState,
  happyHop,
  roamerPose,
  type RoamerEnv,
  type RoamerState
} from './agent-roamer'

type Sprite = NonNullable<CustomPet['sprite']>

const FLOOR_MARGIN = 6

// Why: the agent mascot is alive — it walks the floor, hops, and trips like
// agentum-www. We run one requestAnimationFrame physics loop, draw the current
// pose to a canvas, and position/flip the canvas imperatively each frame so
// React never re-renders at 60fps. The pet sits in a full-viewport
// pointer-events-none layer; only the sprite itself opts back in so app chrome
// stays clickable.
export function AgentRoamer({ url, sprite }: { url: string; sprite: Sprite }): React.JSX.Element {
  const documentVisible = useDocumentVisible()
  const size = useAppStore((s) => s.petSize)

  const petRef = useRef<HTMLDivElement | null>(null)
  const canvasRef = useRef<HTMLCanvasElement | null>(null)
  const imgRef = useRef<HTMLImageElement | null>(null)
  const imgReadyRef = useRef(false)
  const stateRef = useRef<RoamerState | null>(null)
  const lastTimeRef = useRef(0)
  // Pointer drag bookkeeping.
  const dragRef = useRef<{ active: boolean; dx: number; moved: boolean }>({
    active: false,
    dx: 0,
    moved: false
  })

  const env = useCallback(
    (): RoamerEnv => ({
      size,
      groundY: Math.max(0, window.innerHeight - size - FLOOR_MARGIN),
      minX: FLOOR_MARGIN,
      maxX: Math.max(FLOOR_MARGIN, window.innerWidth - size - FLOOR_MARGIN)
    }),
    [size]
  )

  // Load the sprite sheet.
  useEffect(() => {
    imgReadyRef.current = false
    const img = new Image()
    img.onload = (): void => {
      imgRef.current = img
      imgReadyRef.current = true
    }
    img.src = url
    return () => {
      img.onload = null
    }
  }, [url])

  // Draw one pose frame + apply the position/flip/squash transform.
  const render = useCallback(
    (s: RoamerState, e: RoamerEnv): void => {
      const canvas = canvasRef.current
      const pet = petRef.current
      const img = imgRef.current
      if (!canvas || !pet) {
        return
      }
      const ctx = canvas.getContext('2d')
      if (img && ctx) {
        ctx.imageSmoothingEnabled = false
        ctx.clearRect(0, 0, canvas.width, canvas.height)
        const fw = sprite.frameWidth
        const fh = sprite.frameHeight
        const scale = Math.min(size / fw, size / fh)
        const w = fw * scale
        const h = fh * scale
        const pose = roamerPose(s)
        // Bottom-align so the feet rest on the floor line.
        ctx.drawImage(img, pose * fw, 0, fw, fh, (size - w) / 2, size - h, w, h)
      }
      const top = e.groundY + s.jz + s.bobY
      // Squash on landing for a little juice; flip horizontally to face travel.
      const scaleX = s.dir * (1 + s.sq * 0.1)
      const scaleY = 1 - s.sq * 0.12
      pet.style.transform = `translate(${s.x.toFixed(1)}px, ${top.toFixed(1)}px) scale(${scaleX.toFixed(3)}, ${scaleY.toFixed(3)})`
    },
    [size, sprite]
  )

  // Why: pause only when the window is hidden (perf). We deliberately do NOT
  // gate on prefers-reduced-motion: this is an opt-in cosmetic pet the user
  // explicitly enabled — a living, walking mascot is the entire feature, so
  // freezing it for the reduced-motion media query would make it look broken.
  const animate = documentVisible

  useEffect(() => {
    const canvas = canvasRef.current
    if (!canvas) {
      return
    }
    canvas.width = size
    canvas.height = size
    if (!stateRef.current) {
      stateRef.current = createRoamerState(env())
    }

    let raf = 0
    let cancelled = false
    const frame = (now: number): void => {
      if (cancelled) {
        return
      }
      const e = env()
      const s = stateRef.current!
      if (animate && !s.dragging) {
        const dt = Math.min(0.04, lastTimeRef.current ? (now - lastTimeRef.current) / 1000 : 0.016)
        stateRef.current = advanceRoamer(s, e, dt)
      }
      lastTimeRef.current = now
      render(stateRef.current, e)
      raf = requestAnimationFrame(frame)
    }

    // Render once even when paused so a reduced-motion / hidden pet still shows.
    render(stateRef.current, env())
    if (animate || imgReadyRef.current === false) {
      lastTimeRef.current = 0
      raf = requestAnimationFrame(frame)
    }
    return () => {
      cancelled = true
      if (raf) {
        cancelAnimationFrame(raf)
      }
    }
  }, [animate, env, render, size])

  const onPointerDown = (event: React.PointerEvent<HTMLDivElement>): void => {
    if (event.button !== 0) {
      return
    }
    const s = stateRef.current
    if (!s) {
      return
    }
    const e = env()
    dragRef.current = { active: true, dx: event.clientX - s.x, moved: false }
    s.dragging = true
    s.vx = 0
    void e
    event.currentTarget.setPointerCapture(event.pointerId)
    event.preventDefault()
  }

  const onPointerMove = (event: React.PointerEvent<HTMLDivElement>): void => {
    const drag = dragRef.current
    const s = stateRef.current
    if (!drag.active || !s) {
      return
    }
    drag.moved = true
    const e = env()
    s.x = Math.min(e.maxX, Math.max(e.minX, event.clientX - drag.dx))
    // Lift toward the pointer; clamp so the pet can't be shoved through the floor.
    s.jz = Math.min(0, event.clientY - s.bobY - (e.groundY + size / 2))
    s.jv = 0
    render(s, e)
  }

  const endDrag = (event: React.PointerEvent<HTMLDivElement>): void => {
    const drag = dragRef.current
    const s = stateRef.current
    if (!drag.active || !s) {
      return
    }
    drag.active = false
    s.dragging = false
    if (event.currentTarget.hasPointerCapture(event.pointerId)) {
      event.currentTarget.releasePointerCapture(event.pointerId)
    }
    if (!drag.moved) {
      // A tap (no drag) — celebrate with a hop.
      stateRef.current = happyHop(s, size)
    } else {
      // Resume roaming from wherever it was dropped; gravity finishes the fall.
      s.idleT = 0.2
      s.targetX = s.x
    }
  }

  return (
    <div aria-hidden className="pointer-events-none fixed inset-0 z-40 overflow-hidden">
      <div
        ref={petRef}
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={endDrag}
        onPointerCancel={endDrag}
        className="pointer-events-auto absolute left-0 top-0 select-none"
        style={{
          width: size,
          height: size,
          transformOrigin: 'bottom center',
          cursor: 'grab',
          touchAction: 'none',
          willChange: 'transform'
        }}
      >
        <canvas
          ref={canvasRef}
          style={{ width: size, height: size, imageRendering: 'pixelated' }}
        />
      </div>
    </div>
  )
}

export default AgentRoamer
