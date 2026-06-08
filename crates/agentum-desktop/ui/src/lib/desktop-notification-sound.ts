const BUILT_IN_SOUND_IDS = [
  'two-tone',
  'bong',
  'thump',
  'blip',
  'sonar',
  'blop',
  'ding',
  'clack',
  'beep'
] as const

type BuiltInSoundId = (typeof BUILT_IN_SOUND_IDS)[number]

function isBuiltInSoundId(id: string): id is BuiltInSoundId {
  return BUILT_IN_SOUND_IDS.includes(id as BuiltInSoundId)
}

// Why: bundle the built-in notification sounds through Vite (root-relative glob
// → ui/resources/...) so each gets a hashed, actually-served URL in both dev and
// the packaged app. The previous raw `/resources/notification-sounds/x.mp3`
// path 404'd because the desktop build has no `public/` dir serving `resources/`
// at the web root — which is why playback failed with "sound could not be
// played". Other assets (logos, pet models) already use this Vite-import pattern.
const BUILT_IN_SOUND_URLS = import.meta.glob<string>(
  '/resources/notification-sounds/*.mp3',
  { eager: true, query: '?url', import: 'default' }
)

function builtInSoundUrl(soundId: BuiltInSoundId): string | null {
  return BUILT_IN_SOUND_URLS[`/resources/notification-sounds/${soundId}.mp3`] ?? null
}

let lastPlayedAt = 0
const DEDUPE_INTERVAL_MS = 300
let cachedAudioElements: Map<string, HTMLAudioElement> = new Map()

function getSoundUrl(soundId: string, customSoundPath?: string | null): string | null {
  if (soundId === 'custom' && customSoundPath) {
    return customSoundPath
  }
  if (isBuiltInSoundId(soundId)) {
    return builtInSoundUrl(soundId)
  }
  return null
}

function getOrCreateAudioElement(soundUrl: string): HTMLAudioElement {
  let audio = cachedAudioElements.get(soundUrl)
  if (!audio) {
    audio = new Audio(soundUrl)
    cachedAudioElements.set(soundUrl, audio)
  }
  return audio
}

export async function playDesktopNotificationSound(
  customSoundId: string | null | undefined,
  customSoundVolume?: number | null,
  customSoundPath?: string | null,
  options?: { force?: boolean }
): Promise<boolean> {
  if (!customSoundId || customSoundId === 'system') {
    return false
  }

  const now = Date.now()
  if (!options?.force && now - lastPlayedAt < DEDUPE_INTERVAL_MS) {
    return false
  }

  const soundUrl = getSoundUrl(customSoundId, customSoundPath)
  if (!soundUrl) {
    console.warn('Unknown notification sound ID:', customSoundId)
    return false
  }

  try {
    const audio = getOrCreateAudioElement(soundUrl)
    audio.volume = Math.max(0, Math.min(1, (customSoundVolume ?? 100) / 100))
    audio.currentTime = 0
    await audio.play()
    lastPlayedAt = now
    return true
  } catch (err) {
    console.warn('Failed to play notification sound:', err)
    return false
  }
}
