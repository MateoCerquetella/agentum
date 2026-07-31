export type E2EConfig = {
  enabled: boolean
  headless: boolean
  exposeStore: boolean
  userDataDir: string | null
}

type E2EConfigInput = {
  headless?: boolean
  exposeStore?: boolean
  userDataDir?: string | null
}

export function createE2EConfig(input: E2EConfigInput | null | undefined): E2EConfig {
  const userDataDir = input?.userDataDir?.trim() || null
  const headless = Boolean(input?.headless)
  const exposeStore = Boolean(input?.exposeStore)

  return {
    enabled: headless || exposeStore || userDataDir !== null,
    headless,
    exposeStore,
    userDataDir
  }
}

export async function loadE2EConfig(
  load: () => E2EConfigInput | null | undefined | PromiseLike<E2EConfigInput | null | undefined>
): Promise<E2EConfig> {
  try {
    return createE2EConfig(await load())
  } catch {
    return createE2EConfig(null)
  }
}
