type SpeechModelType = 'transducer' | 'paraformer' | 'whisper'

type ModelingUnit = 'bpe' | 'cjkchar' | 'cjkchar+bpe'

export type SpeechModelManifest = {
  id: string
  label: string
  description: string
  type: SpeechModelType
  language: string
  sizeBytes: number
  downloadUrl: string
  archiveSha256: string
  archiveFormat: 'tar.bz2'
  files: string[]
  sampleRate: number
  streaming: boolean
  modelingUnit?: ModelingUnit
  recommended?: boolean
}

type SpeechModelStatus = 'not-downloaded' | 'downloading' | 'extracting' | 'ready' | 'error'

export type SpeechModelState = {
  id: string
  status: SpeechModelStatus
  progress?: number
  error?: string
}

type SpeechTranscriptEvent = {
  text: string
  sessionId: string
}

type SpeechLifecycleEvent = {
  sessionId: string
}

type SpeechErrorEvent = {
  error: string
  sessionId: string
}

export type DictationState = 'idle' | 'starting' | 'listening' | 'stopping' | 'error'

type UserModelConfig = {
  id: string
  type: SpeechModelType
  dir: string
  sampleRate?: number
}

type DictationMode = 'toggle' | 'hold'

export type VoiceSettings = {
  enabled: boolean
  sttModel: string
  modelsDir: string
  language: string
  dictationMode: DictationMode
  terminalConfirmBeforeInsert: boolean
  userModels: UserModelConfig[]
}
