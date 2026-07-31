import { api } from '@/tauri'
import { createE2EConfig, loadE2EConfig, type E2EConfig } from '@/shared/e2e-config'

// Why: preload owns the Electron startup contract, so renderer code should
// consume the bridged E2E config from api instead of reading env vars.
export const e2eConfig: E2EConfig =
  typeof window !== 'undefined' && api?.e2e
    ? await loadE2EConfig(() => api.e2e.getConfig())
    : createE2EConfig(null)
