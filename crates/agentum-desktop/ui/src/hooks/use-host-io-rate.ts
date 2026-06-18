import { useEffect, useRef, useState } from 'react'
import { rateFromSamples, snapshot, type HostKey } from '@/runtime/io-meter'

export type HostIoRate = {
  /** Bytes/sec received over the WS for this host (server → client). */
  inRate: number
  /** Bytes/sec sent over the WS for this host (client → server). */
  outRate: number
}

/** A cumulative byte total tagged with the time it was read. */
export type IoSample = { bytes: number; at: number }

export type HostIoSamplerState = {
  prevIn: IoSample | null
  prevOut: IoSample | null
}

/** How often we resample the host's cumulative counters. ~1s matches the
 *  perceived cadence of the TUI meter and keeps React re-renders to one per
 *  second regardless of how busy the byte stream is. */
const SAMPLE_INTERVAL_MS = 1000

/**
 * Pure one-tick sampler: given the prior in/out samples and the host's current
 * cumulative totals (`bytesIn`/`bytesOut`) read at time `at`, derive the in/out
 * rates and the next sampler state. Extracted so the rate logic is testable
 * without a React renderer. First tick (null priors) yields a 0 rate baseline.
 */
export function sampleHostIoRate(
  state: HostIoSamplerState,
  current: { bytesIn: number; bytesOut: number },
  at: number
): { rate: HostIoRate; next: HostIoSamplerState } {
  const nextIn: IoSample = { bytes: current.bytesIn, at }
  const nextOut: IoSample = { bytes: current.bytesOut, at }
  return {
    rate: {
      inRate: rateFromSamples(state.prevIn, nextIn),
      outRate: rateFromSamples(state.prevOut, nextOut)
    },
    next: { prevIn: nextIn, prevOut: nextOut }
  }
}

/**
 * Sample a host's cumulative WS byte counters on a fixed interval and return the
 * derived throughput. Mirrors the TUI iometer's Δbytes/Δtime rate; the first
 * tick after mount (or after a host switch) reports 0 because there's no prior
 * sample to diff against.
 *
 * Cheap by construction: the hot path (every WS frame) only mutates integer
 * counters in io-meter.ts. This hook merely reads two ints per second.
 */
export function useHostIoRate(hostKey: HostKey): HostIoRate {
  const [rate, setRate] = useState<HostIoRate>({ inRate: 0, outRate: 0 })
  // Last samples, kept in a ref so the interval closure always sees the latest
  // without re-subscribing. Reset when the selected host changes.
  const samplerRef = useRef<HostIoSamplerState>({ prevIn: null, prevOut: null })

  useEffect(() => {
    // New host → drop prior samples so the first tick doesn't diff across hosts.
    samplerRef.current = { prevIn: null, prevOut: null }
    setRate({ inRate: 0, outRate: 0 })

    const tick = (): void => {
      const { rate: nextRate, next } = sampleHostIoRate(
        samplerRef.current,
        snapshot(hostKey),
        Date.now()
      )
      samplerRef.current = next
      setRate((current) =>
        current.inRate === nextRate.inRate && current.outRate === nextRate.outRate
          ? current
          : nextRate
      )
    }

    const handle = window.setInterval(tick, SAMPLE_INTERVAL_MS)
    return () => window.clearInterval(handle)
  }, [hostKey])

  return rate
}
