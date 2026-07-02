type PasteTextOptions = {
  forceBracketedPaste?: boolean
}

type SaveClipboardImageAsTempFile = (args?: {
  connectionId?: string | null
}) => Promise<string | null>

type PasteTerminalClipboardDeps = {
  readClipboardText: () => Promise<string>
  saveClipboardImageAsTempFile: SaveClipboardImageAsTempFile
  pasteText: (text: string, options?: PasteTextOptions) => void
  connectionId?: string | null
  onImagePasteError?: (error: unknown) => void
  // Why: on an SSH worktree the saved clipboard image is a LOCAL temp path the
  // remote agent can't read. When provided, deliver the image to the remote
  // instead of pasting the path (the server writes it remote-side and types the
  // path itself). Throwing routes to onImagePasteError.
  uploadImageForRemote?: (localPath: string) => Promise<void>
}

export async function pasteTerminalClipboard({
  readClipboardText,
  saveClipboardImageAsTempFile,
  pasteText,
  connectionId,
  onImagePasteError,
  uploadImageForRemote
}: PasteTerminalClipboardDeps): Promise<void> {
  let text = ''
  try {
    text = await readClipboardText()
  } catch {
    // Why: browser clipboard text reads can fail for image-only clipboards.
    // Still try the image path so Cmd/Ctrl+V works for screenshots.
  }
  if (text) {
    pasteText(text)
    return
  }

  try {
    const filePath = await saveClipboardImageAsTempFile({ connectionId })
    if (!filePath) {
      return
    }
    if (uploadImageForRemote) {
      // SSH worktree: the local path is unreachable to the remote agent. Upload
      // the bytes to the remote workdir; the server types the path itself, so we
      // don't also paste here.
      await uploadImageForRemote(filePath)
      return
    }
    pasteText(filePath, {
      // Why: a generated clipboard-image path is terminal image injection, not
      // ordinary one-line text. Keep it off the Ctrl+C stale-text paste path.
      forceBracketedPaste: true
    })
  } catch (error) {
    onImagePasteError?.(error)
  }
}
