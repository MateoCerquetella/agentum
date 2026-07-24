import type { SddInjectMode } from '@/runtime/sdd-client'

export type SddDeliveryResult = { mode: SddInjectMode; ready: boolean }

/** Drive the toolbar's pending/result state around one confirmed delivery. */
export async function deliverSddPlaybook(args: {
  title: string
  inject: () => Promise<SddDeliveryResult>
  setSending: (sending: boolean) => void
  setNotice: (notice: string) => void
}): Promise<void> {
  args.setSending(true)
  try {
    const { mode, ready } = await args.inject()
    const delivered =
      mode === 'bootstrap' ? `${args.title} sent via MCP` : `${args.title} sent (full text)`
    args.setNotice(ready ? delivered : `${delivered}; readiness was not confirmed`)
  } catch (error) {
    const detail = error instanceof Error ? `: ${error.message}` : ''
    args.setNotice(`Could not inject ${args.title}${detail}`)
  } finally {
    args.setSending(false)
  }
}
