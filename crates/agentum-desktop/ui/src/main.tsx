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
import { logEmbeddedServerSnapshot } from './runtime/agentum-server-client'

// Exercise the embedded agentum-server over its session model (the shared core
// the TUI uses) on boot. Non-blocking; this is the desktop's path to Option A
// session-per-workspace.
void logEmbeddedServerSnapshot()

recordRendererCrashBreadcrumb('renderer_bootstrap_started', { dev: import.meta.env.DEV })
installRendererCrashDiagnostics()

// Apply the last-used theme immediately so the first paint matches the user's
// choice (no dark flash on light setups). App.tsx re-applies authoritatively once
// settings load, and persists `agentum-theme` on every change.
const persistedTheme =
  (localStorage.getItem('agentum-theme') as 'system' | 'dark' | 'light' | null) ?? 'system'
applyDocumentTheme(persistedTheme, { disableTransitions: false })

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
