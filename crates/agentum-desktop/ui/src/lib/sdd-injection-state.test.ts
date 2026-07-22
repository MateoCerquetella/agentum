import { describe, expect, it, vi } from 'vitest'
import { deliverSddPlaybook, type SddDeliveryResult } from './sdd-injection-state'

function deferred<T>(): {
  promise: Promise<T>
  resolve: (value: T) => void
  reject: (error: unknown) => void
} {
  let resolve!: (value: T) => void
  let reject!: (error: unknown) => void
  const promise = new Promise<T>((res, rej) => {
    resolve = res
    reject = rej
  })
  return { promise, resolve, reject }
}

describe('deliverSddPlaybook', () => {
  it('stays pending until confirmed success and reports readiness', async () => {
    const delivery = deferred<SddDeliveryResult>()
    const setSending = vi.fn()
    const setNotice = vi.fn()
    const pending = deliverSddPlaybook({
      title: 'Continue',
      inject: () => delivery.promise,
      setSending,
      setNotice
    })

    expect(setSending).toHaveBeenLastCalledWith(true)
    expect(setNotice).not.toHaveBeenCalled()
    delivery.resolve({ mode: 'bootstrap', ready: true })
    await pending

    expect(setNotice).toHaveBeenCalledWith('Continue sent via MCP')
    expect(setSending).toHaveBeenLastCalledWith(false)
  })

  it('reports a delivery rejection and clears pending state', async () => {
    const setSending = vi.fn()
    const setNotice = vi.fn()

    await deliverSddPlaybook({
      title: 'Spec',
      inject: () => Promise.reject(new Error('session stopped')),
      setSending,
      setNotice
    })

    expect(setNotice).toHaveBeenCalledWith('Could not inject Spec: session stopped')
    expect(setSending).toHaveBeenLastCalledWith(false)
  })
})
