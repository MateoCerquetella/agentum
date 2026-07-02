import { api } from '@/tauri'
import { base64ToBytes } from '@/shared/base64'
import { uploadSessionImage } from '@/runtime/agentum-server-client'
import { useAppStore } from '@/store'

/**
 * Resolve the server SESSION id for a worktree's active agent pane from its
 * registered `server:<sessionId>:<leafId>` ptyId. Server-session panes (every
 * agent terminal) have no `PtyTransport`, so this ptyId — stored in
 * `ptyIdsByTabId` by `server-pane-connection.ts` — is the only handle on the
 * session id, which we need to POST a screenshot to `/api/sessions/{id}/uploads`.
 * Both ids are colon-free UUIDs, so `split(':')[1]` is unambiguous.
 */
export function resolveServerSessionId(tabId: string, leafId: string): string | null {
  const ptyIds = useAppStore.getState().ptyIdsByTabId[tabId] ?? []
  const match =
    ptyIds.find((p) => p.startsWith('server:') && p.endsWith(`:${leafId}`)) ??
    ptyIds.find((p) => p.startsWith('server:'))
  return match ? (match.split(':')[1] ?? null) : null
}

type FsReadResult = { content: string; isBinary?: boolean; mimeType?: string }

/**
 * Read a LOCAL file's bytes for upload. The fs command returns binary content
 * base64-encoded; we decode it back to raw bytes. (SSH transport isn't ported in
 * that command, but the source here is always a local file — a clipboard temp
 * file or a dragged-in path — so local read is correct.)
 */
async function readLocalFileBytes(
  filePath: string
): Promise<{ bytes: Uint8Array; contentType: string }> {
  const result = (await api.fs.readFile({ filePath })) as FsReadResult
  const bytes = base64ToBytes(result.content)
  return { bytes, contentType: result.mimeType ?? 'application/octet-stream' }
}

/**
 * Deliver a local image file to a worktree's REMOTE agent: read its bytes and
 * POST them to the host-aware `/api/sessions/{id}/uploads` route, which writes
 * the file onto the remote host and types the path into the remote pane. The
 * server does the injection, so callers must NOT also client-paste the path.
 * Throws on failure (no session, read error, HTTP error) for the caller to
 * surface.
 */
export async function uploadLocalImageToSession(sessionId: string, filePath: string): Promise<void> {
  const { bytes, contentType } = await readLocalFileBytes(filePath)
  await uploadSessionImage(sessionId, bytes, contentType)
}
