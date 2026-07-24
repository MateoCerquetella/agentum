const AGENTUM_SKILLS_REPOSITORY_URL = 'https://github.com/mateocerquetella/agentum'

export const BROWSER_VERIFICATION_LOOP_SKILL_NAME = 'browser-verification-loop'

export function buildAgentFeatureSkillInstallCommand(skillNames: readonly string[]): string {
  if (skillNames.length === 0) {
    throw new Error('At least one skill name is required.')
  }
  return `npx skills add ${AGENTUM_SKILLS_REPOSITORY_URL} --skill ${skillNames.join(' ')} --global`
}

export const BROWSER_VERIFICATION_LOOP_SKILL_INSTALL_COMMAND =
  buildAgentFeatureSkillInstallCommand([BROWSER_VERIFICATION_LOOP_SKILL_NAME])
