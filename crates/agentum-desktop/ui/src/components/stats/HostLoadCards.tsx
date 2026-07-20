import { useEffect, useState } from 'react'
import { Cpu, MemoryStick } from 'lucide-react'
import { StatCard } from './StatCard'
import { apiUrl, getServerEndpoint } from '@/runtime/server-endpoint'
import { subscribeServerEvents } from '@/runtime/server-events-bus'

// #262 — the previously-orphaned `GET /api/host/metrics` gets its consumer.
// Lives inside the SHARED StatsPane so Settings → Stats & Usage and Mission
// Control show identical host numbers by construction. Initial paint comes
// from the GET snapshot; live updates ride the shared `/api/events` socket's
// `host.metrics` frames (a 2 s server-side ticker — no per-tab poller).

type HostMetrics = {
  cpu_pct: number
  mem_used: number
  mem_total: number
  cpu_count: number
}

function parseMetrics(raw: unknown): HostMetrics | null {
  if (!raw || typeof raw !== 'object') return null
  const m = raw as Record<string, unknown>
  if (
    typeof m.cpu_pct !== 'number' ||
    typeof m.mem_used !== 'number' ||
    typeof m.mem_total !== 'number'
  ) {
    return null
  }
  return {
    cpu_pct: m.cpu_pct,
    mem_used: m.mem_used,
    mem_total: m.mem_total,
    cpu_count: typeof m.cpu_count === 'number' ? m.cpu_count : 0
  }
}

function formatGb(bytes: number): string {
  return (bytes / 1024 ** 3).toFixed(1)
}

export function HostLoadCards(): React.JSX.Element | null {
  const [metrics, setMetrics] = useState<HostMetrics | null>(null)

  useEffect(() => {
    let cancelled = false
    // One snapshot for the first paint — the event stream then keeps it live.
    void (async () => {
      try {
        const url = await apiUrl('/api/host/metrics')
        const { token } = await getServerEndpoint()
        const res = await fetch(url, {
          headers: token ? { Authorization: `Bearer ${token}` } : {}
        })
        if (!res.ok) return
        const parsed = parseMetrics(await res.json())
        if (!cancelled && parsed) setMetrics(parsed)
      } catch {
        // Host load is decorative — an unreachable endpoint just hides the row.
      }
    })()
    const unsubscribe = subscribeServerEvents({
      onEvent: (ev) => {
        if (ev.kind !== 'host.metrics') return
        const parsed = parseMetrics(ev.payload)
        if (parsed) setMetrics(parsed)
      }
    })
    return () => {
      cancelled = true
      unsubscribe()
    }
  }, [])

  if (!metrics) return null

  return (
    <div className="grid grid-cols-2 gap-3">
      <StatCard
        label={metrics.cpu_count > 0 ? `Host CPU · ${metrics.cpu_count} cores` : 'Host CPU'}
        value={`${Math.round(metrics.cpu_pct)}%`}
        icon={<Cpu className="size-4" />}
      />
      <StatCard
        label="Host memory"
        value={`${formatGb(metrics.mem_used)} / ${formatGb(metrics.mem_total)} GB`}
        icon={<MemoryStick className="size-4" />}
      />
    </div>
  )
}
