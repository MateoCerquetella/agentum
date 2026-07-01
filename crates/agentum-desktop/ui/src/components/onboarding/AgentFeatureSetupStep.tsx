import { FeatureSetupChecklist } from './FeatureSetupChecklist'
import type { OnboardingFeatureSetupSelection } from './onboarding-feature-setup'

type AgentFeatureSetupStepProps = {
  featureSetup: OnboardingFeatureSetupSelection
  onFeatureSetupChange: (value: OnboardingFeatureSetupSelection) => void
}

// The agentum MCP tools apply when the user continues (see `next` in
// use-onboarding-flow), so this step is just the toggle list — no separate
// "enable" button and no skill-install terminal.
export function AgentFeatureSetupStep({
  featureSetup,
  onFeatureSetupChange
}: AgentFeatureSetupStepProps): React.JSX.Element {
  return <FeatureSetupChecklist value={featureSetup} onChange={onFeatureSetupChange} />
}
