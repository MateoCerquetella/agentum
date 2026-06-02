import { track } from '@/lib/telemetry'
import type { EventProps } from '../../../../shared/telemetry-events'

export type AgentumCliFeatureTipSource = EventProps<'agentum_cli_feature_tip_shown'>['source']
export type AgentumCliFeatureTipSetupResult = EventProps<'agentum_cli_feature_tip_setup_result'>['result']

export function getAgentumCliFeatureTipTelemetrySource(value: unknown): AgentumCliFeatureTipSource {
  return value === 'app_open' ? 'app_open' : 'manual'
}

export function trackAgentumCliFeatureTipShown(source: AgentumCliFeatureTipSource): void {
  track('agentum_cli_feature_tip_shown', { source })
}

export function trackAgentumCliFeatureTipSetupClicked(source: AgentumCliFeatureTipSource): void {
  track('agentum_cli_feature_tip_setup_clicked', { source })
}

export function trackAgentumCliFeatureTipSetupResult(
  source: AgentumCliFeatureTipSource,
  result: AgentumCliFeatureTipSetupResult
): void {
  track('agentum_cli_feature_tip_setup_result', { source, result })
}
