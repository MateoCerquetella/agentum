export const AGENTUM_RUNTIME_RPC_FEATURE_INTERACTION_SOURCE_KEY = '__agentumFeatureInteractionSource'

export const AGENTUM_RUNTIME_RPC_BROWSER_UI_SOURCE = 'browser-pane-ui'

export function withBrowserPaneUiRuntimeRpcSource(value: unknown): unknown {
  if (value === null || typeof value !== 'object' || Array.isArray(value)) {
    return {
      [AGENTUM_RUNTIME_RPC_FEATURE_INTERACTION_SOURCE_KEY]: AGENTUM_RUNTIME_RPC_BROWSER_UI_SOURCE
    }
  }
  return {
    ...value,
    [AGENTUM_RUNTIME_RPC_FEATURE_INTERACTION_SOURCE_KEY]: AGENTUM_RUNTIME_RPC_BROWSER_UI_SOURCE
  }
}

function isBrowserPaneUiRuntimeRpcParams(value: unknown): boolean {
  return (
    value !== null &&
    typeof value === 'object' &&
    !Array.isArray(value) &&
    (value as Record<string, unknown>)[AGENTUM_RUNTIME_RPC_FEATURE_INTERACTION_SOURCE_KEY] ===
      AGENTUM_RUNTIME_RPC_BROWSER_UI_SOURCE
  )
}
