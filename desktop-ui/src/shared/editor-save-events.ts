export const AGENTUM_EDITOR_SAVE_DIRTY_FILES_EVENT = 'agentum:editor-save-dirty-files'

export type EditorSaveDirtyFilesDetail = {
  claim: () => void
  resolve: () => void
  reject: (message: string) => void
}
