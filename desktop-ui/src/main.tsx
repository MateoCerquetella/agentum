import './lib/electron-bridge'
import './assets/main.css'

import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import App from './App'
import { RecoverableRenderErrorBoundary } from './components/error-boundaries/RecoverableRenderErrorBoundary'
import {
  installRendererCrashDiagnostics,
  recordRendererCrashBreadcrumb
} from './lib/crash-diagnostics'
import { applyDocumentTheme } from './lib/document-theme'
import { shouldEnableReactGrab } from './lib/react-grab-dev-gate'
import { logEmbeddedServerSnapshot } from './runtime/agentum-server-client'

// Exercise the embedded agentum-server over its session model (the shared core
// the TUI uses) on boot. Non-blocking; this is the desktop's path to Option A
// session-per-workspace.
void logEmbeddedServerSnapshot()

recordRendererCrashBreadcrumb('renderer_bootstrap_started', { dev: import.meta.env.DEV })
installRendererCrashDiagnostics()

if (
  import.meta.env.DEV &&
  shouldEnableReactGrab({
    dev: import.meta.env.DEV,
    enableFlag: import.meta.env.VITE_ENABLE_REACT_GRAB
  })
) {
  void import('react-grab').then(({ init }) => init())
  void import('react-grab/styles.css')
}

applyDocumentTheme('dark', { disableTransitions: false })

const rootElement = document.getElementById('root')
if (!rootElement) {
  recordRendererCrashBreadcrumb('renderer_root_missing')
  throw new Error('Renderer root element not found.')
}

createRoot(rootElement).render(
  <StrictMode>
    <RecoverableRenderErrorBoundary
      boundaryId="app.root"
      surface="app-root"
      title="agentum hit a renderer error."
      description="The app shell could not finish rendering. Retry to remount it, or relaunch agentum if the error persists."
    >
      <App />
    </RecoverableRenderErrorBoundary>
  </StrictMode>
)
recordRendererCrashBreadcrumb('renderer_bootstrap_rendered')
