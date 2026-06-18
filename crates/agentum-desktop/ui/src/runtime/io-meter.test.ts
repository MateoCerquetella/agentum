import { afterEach, describe, expect, it } from 'vitest'
import {
  formatRate,
  knownHostKeys,
  rateFromSamples,
  record,
  resetAll,
  snapshot,
  LOCAL_HOST_KEY
} from './io-meter'

afterEach(() => {
  resetAll()
})

describe('io-meter counters', () => {
  it('reports zeros for an unseen host', () => {
    expect(snapshot('ssh:nope')).toEqual({ bytesIn: 0, bytesOut: 0 })
  })

  it('accumulates inbound and outbound separately per host', () => {
    record(LOCAL_HOST_KEY, { in: 100 })
    record(LOCAL_HOST_KEY, { out: 25 })
    record(LOCAL_HOST_KEY, { in: 50, out: 5 })
    expect(snapshot(LOCAL_HOST_KEY)).toEqual({ bytesIn: 150, bytesOut: 30 })
  })

  it('keeps host buckets independent', () => {
    record(LOCAL_HOST_KEY, { in: 10 })
    record('ssh:abc', { in: 999, out: 7 })
    expect(snapshot(LOCAL_HOST_KEY)).toEqual({ bytesIn: 10, bytesOut: 0 })
    expect(snapshot('ssh:abc')).toEqual({ bytesIn: 999, bytesOut: 7 })
  })

  it('ignores zero, negative, and non-finite deltas', () => {
    record(LOCAL_HOST_KEY, { in: 0, out: 0 })
    record(LOCAL_HOST_KEY, { in: -5 })
    record(LOCAL_HOST_KEY, { out: Number.NaN })
    record(LOCAL_HOST_KEY, { in: Number.POSITIVE_INFINITY })
    expect(snapshot(LOCAL_HOST_KEY)).toEqual({ bytesIn: 0, bytesOut: 0 })
    // A host that only ever saw no-op records still shouldn't be created.
    expect(knownHostKeys()).not.toContain(LOCAL_HOST_KEY)
  })

  it('tracks which hosts have recorded traffic', () => {
    record('ssh:one', { in: 1 })
    record('ssh:two', { out: 1 })
    expect(new Set(knownHostKeys())).toEqual(new Set(['ssh:one', 'ssh:two']))
  })

  it('returns a copy from snapshot so callers cannot mutate counters', () => {
    record(LOCAL_HOST_KEY, { in: 100 })
    const snap = snapshot(LOCAL_HOST_KEY)
    snap.bytesIn = 0
    expect(snapshot(LOCAL_HOST_KEY).bytesIn).toBe(100)
  })
})

describe('rateFromSamples', () => {
  it('returns 0 on the first sample (no prior)', () => {
    expect(rateFromSamples(null, { bytes: 100, at: 1000 })).toBe(0)
  })

  it('computes bytes/sec from the delta over the span', () => {
    // 1000 bytes over 1000ms = 1000 B/s.
    expect(rateFromSamples({ bytes: 0, at: 0 }, { bytes: 1000, at: 1000 })).toBe(1000)
    // 2048 bytes over 500ms = 4096 B/s.
    expect(rateFromSamples({ bytes: 0, at: 0 }, { bytes: 2048, at: 500 })).toBe(4096)
  })

  it('returns 0 when bytes did not advance (idle window)', () => {
    expect(rateFromSamples({ bytes: 500, at: 0 }, { bytes: 500, at: 1000 })).toBe(0)
  })

  it('returns 0 on a counter reset (delta < 0)', () => {
    expect(rateFromSamples({ bytes: 500, at: 0 }, { bytes: 0, at: 1000 })).toBe(0)
  })

  it('floors the span at 1ms to avoid divide-by-zero', () => {
    // Same timestamp → 1ms span → 10 bytes * 1000 = 10000 B/s, finite.
    const rate = rateFromSamples({ bytes: 0, at: 5 }, { bytes: 10, at: 5 })
    expect(Number.isFinite(rate)).toBe(true)
    expect(rate).toBe(10000)
  })
})

describe('formatRate', () => {
  it('renders an em dash below 1 B/s', () => {
    expect(formatRate(0)).toBe('—')
    expect(formatRate(0.4)).toBe('—')
    expect(formatRate(Number.NaN)).toBe('—')
  })

  it('uses B/s under 1 KiB', () => {
    expect(formatRate(50)).toBe('50 B/s')
    expect(formatRate(1)).toBe('1.0 B/s')
    expect(formatRate(999)).toBe('999 B/s')
  })

  it('uses KiB/s between 1 KiB and 1 MiB', () => {
    expect(formatRate(2048)).toBe('2.0 KiB/s')
    expect(formatRate(1024 * 50)).toBe('50 KiB/s')
  })

  it('uses MiB/s between 1 MiB and 1 GiB', () => {
    expect(formatRate(1024 * 1024 * 5)).toBe('5.0 MiB/s')
  })

  it('uses GiB/s at and above 1 GiB', () => {
    expect(formatRate(1024 * 1024 * 1024 * 2)).toBe('2.0 GiB/s')
  })

  it('drops the decimal for values >= 10', () => {
    expect(formatRate(12345)).toBe('12 KiB/s')
  })
})
