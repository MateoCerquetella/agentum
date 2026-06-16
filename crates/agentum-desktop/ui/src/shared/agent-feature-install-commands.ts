export const AGENTUM_SKILLS_REPOSITORY_URL = 'https://github.com/mateocerquetella/agentum'

export const AGENTUM_CLI_SKILL_NAME = 'agentum-cli'
export const COMPUTER_USE_SKILL_NAME = 'computer-use'
export const ORCHESTRATION_SKILL_NAME = 'orchestration'
export const BROWSER_VERIFICATION_LOOP_SKILL_NAME = 'browser-verification-loop'

export function buildAgentFeatureSkillInstallCommand(skillNames: readonly string[]): string {
  if (skillNames.length === 0) {
    throw new Error('At least one skill name is required.')
  }
  return `npx skills add ${AGENTUM_SKILLS_REPOSITORY_URL} --skill ${skillNames.join(' ')} --global`
}

export const AGENTUM_CLI_SKILL_INSTALL_COMMAND = buildAgentFeatureSkillInstallCommand([
  AGENTUM_CLI_SKILL_NAME
])

export const COMPUTER_USE_SKILL_INSTALL_COMMAND = buildAgentFeatureSkillInstallCommand([
  COMPUTER_USE_SKILL_NAME
])

export const ORCHESTRATION_SKILL_INSTALL_COMMAND = buildAgentFeatureSkillInstallCommand([
  ORCHESTRATION_SKILL_NAME
])

export const AGENTUM_CLI_ORCHESTRATION_SKILL_INSTALL_COMMAND = buildAgentFeatureSkillInstallCommand([
  AGENTUM_CLI_SKILL_NAME,
  ORCHESTRATION_SKILL_NAME
])

export const BROWSER_VERIFICATION_LOOP_SKILL_INSTALL_COMMAND =
  buildAgentFeatureSkillInstallCommand([BROWSER_VERIFICATION_LOOP_SKILL_NAME])
