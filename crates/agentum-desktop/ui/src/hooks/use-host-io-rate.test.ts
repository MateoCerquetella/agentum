import { describe, expect, it } from 'vitest'
import { sampleHostIoRate, type HostIoSamplerState } from './use-host-io-rate'

const EMPTY: HostIoSamplerState = { prevIn: null, prevOut: null }

describe('sampleHostIoRate', () => {
  it('reports 0 on the first tick (no prior samples) and seeds the baseline', () => {
    const { rate, next } = sampleHostIoRate(EMPTY, { bytesIn: 1000, bytesOut: 50 }, 1000)
    expect(rate).toEqual({ inRate: 0, outRate: 0 })
    // Baseline captured so the next tick can diff against it.
    expect(next).toEqual({
      prevIn: { bytes: 1000, at: 1000 },
      prevOut: { bytes: 50, at: 1000 }
    })
  })

  it('derives bytes/sec from the delta over the span between ticks', () => {
    const first = sampleHostIoRate(EMPTY, { bytesIn: 0, bytesOut: 0 }, 0)
    // 2048 bytes in / 64 out arrive over a 1000ms window.
    const second = sampleHostIoRate(first.next, { bytesIn: 2048, bytesOut: 64 }, 1000)
    expect(second.rate).toEqual({ inRate: 2048, outRate: 64 })
  })

  it('reports 0 for a host that received nothing between ticks', () => {
    const first = sampleHostIoRate(EMPTY, { bytesIn: 500, bytesOut: 10 }, 0)
    const second = sampleHostIoRate(first.next, { bytesIn: 500, bytesOut: 10 }, 1000)
    expect(second.rate).toEqual({ inRate: 0, outRate: 0 })
  })

  it('handles independent in/out rates within one tick', () => {
    const first = sampleHostIoRate(EMPTY, { bytesIn: 0, bytesOut: 0 }, 0)
    // 4096 in over 500ms = 8192 B/s; 0 out → 0 B/s.
    const second = sampleHostIoRate(first.next, { bytesIn: 4096, bytesOut: 0 }, 500)
    expect(second.rate.inRate).toBe(8192)
    expect(second.rate.outRate).toBe(0)
  })
})
