export type ShellOpenLocalPathFailureReason = 'not-absolute' | 'not-found' | 'launch-failed'

type ShellOpenLocalPathResult =
  | { ok: true }
  | { ok: false; reason: ShellOpenLocalPathFailureReason }
