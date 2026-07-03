import { useEffect, useRef, useState } from 'react'
import type { CustomPet } from '../../../../shared/types'
import { useAppStore } from '../../store'
import { BUNDLED_PET, findBundledPet, isBundledPetId } from './pet-models'
import {
  blobUrlCache,
  detectedSpriteCache,
  loadCustomBlobUrl,
  type DetectedSpriteCacheEntry
} from './pet-blob-cache'

// Re-export so existing callers (the store slice) that point at this module
// keep working without knowing about the cache module split.


export type ResolvedPet = {
  url: string
  ready: boolean
  sprite: NonNullable<CustomPet['sprite']> | null
  detected: DetectedSpriteCacheEntry | null
  // Why: when set, the overlay routes to the dedicated behavior renderer
  // (AgentRoamer) instead of the CSS frame-step / flat-image paths.
  behavior: 'agent' | null
}

/** Resolve the active pet to a URL the overlay can render.
 *
 *  For bundled pets this is synchronous. For custom ones we issue an
 *  IPC read and build a blob: URL with the correct MIME; until that resolves,
 *  we fall back to the bundled default so the overlay is never empty.
 */
export function usePetUrl(): ResolvedPet {
  const petId = useAppStore((s) => s.petId)
  const customPets = useAppStore((s) => s.customPets)
  const bundled = isBundledPetId(petId)
  const customMeta = bundled ? null : customPets.find((m) => m.id === petId)

  const [customUrl, setCustomUrl] = useState<string | null>(() =>
    customMeta ? (blobUrlCache.get(customMeta.id) ?? null) : null
  )
  // Why: track the last id we started loading so a rapid switch between
  // custom pets doesn't let a slower earlier response clobber the newer
  // state.
  const pendingRef = useRef<string | null>(null)

  const customId = customMeta?.id ?? null
  const customFileName = customMeta?.fileName ?? null
  const customMime = customMeta?.mimeType ?? 'image/png'
  const customKind = customMeta?.kind ?? 'image'
  // Why: prefer manifest fps captured at import time; sprite-with-frame entries
  // store fps on `sprite`, frame-less bundles carry it on `spriteFps`.
  const customSpriteFps = customMeta?.sprite?.fps ?? customMeta?.spriteFps
  // Why: when the manifest already declares a valid sprite layout, the
  // overlay reads the `sprite` branch and never touches detectedSpriteCache,
  // so we skip auto-detection in the cache loader to avoid leaking ImageBitmaps.
  const customHasManifestSprite =
    !!customMeta?.sprite &&
    customMeta.sprite.frameWidth > 0 &&
    customMeta.sprite.frameHeight > 0 &&
    customMeta.sprite.fps > 0
  useEffect(() => {
    if (!customId || !customFileName) {
      setCustomUrl(null)
      return
    }
    const cached = blobUrlCache.get(customId)
    if (cached) {
      setCustomUrl(cached)
      return
    }
    // Why: clear the previous custom blob URL before awaiting the new one so
    // the hook's fallback-to-bundled branch kicks in during the load window.
    setCustomUrl(null)
    pendingRef.current = customId
    let cancelled = false
    void loadCustomBlobUrl(
      customId,
      customFileName,
      customMime,
      customKind,
      customSpriteFps,
      customHasManifestSprite
    ).then((url) => {
      if (cancelled || pendingRef.current !== customId) {
        return
      }
      setCustomUrl(url)
    })
    return () => {
      cancelled = true
    }
  }, [customId, customFileName, customMime, customKind, customSpriteFps, customHasManifestSprite])

  if (bundled) {
    const pet = findBundledPet(petId) ?? BUNDLED_PET
    // Why: bundled pets may ship sprite-sheet metadata (the agent mascot does)
    // so the overlay can crop poses; `behavior` routes it to the dedicated
    // behavior renderer. Flat-image bundles leave both undefined and fall
    // through to the <img> branch.
    return {
      url: pet.url,
      ready: true,
      sprite: pet.sprite ?? null,
      detected: null,
      behavior: pet.behavior ?? null
    }
  }
  if (customMeta && customUrl) {
    // Why: guard against manifest entries with zero/negative dims or fps —
    // those would break the overlay's frame math, so fall through to detection.
    if (
      customMeta.sprite &&
      customMeta.sprite.frameWidth > 0 &&
      customMeta.sprite.frameHeight > 0 &&
      customMeta.sprite.fps > 0
    ) {
      return { url: customUrl, ready: true, sprite: customMeta.sprite, detected: null, behavior: null }
    }
    const detected = detectedSpriteCache.get(customMeta.id)
    if (detected) {
      return { url: customUrl, ready: true, sprite: null, detected, behavior: null }
    }
    return { url: customUrl, ready: true, sprite: null, detected: null, behavior: null }
  }
  // Why: while a custom pet's blob is still loading we fall back to the bundled
  // default. Surface its sprite + behavior too so the loading window animates
  // the same way instead of flashing the raw multi-pose strip.
  return {
    url: BUNDLED_PET.url,
    ready: false,
    sprite: BUNDLED_PET.sprite ?? null,
    detected: null,
    behavior: BUNDLED_PET.behavior ?? null
  }
}
