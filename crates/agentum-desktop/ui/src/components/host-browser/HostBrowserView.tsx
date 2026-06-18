// Top-level "Host Browser" view (spec 009a Phase 3): pick an SSH host + a
// worktree dir, then watch a headless Chromium running ON that host live. The
// picker is intentionally simple — the host browser is keyed per worktree, so
// reopening with the same host+dir re-attaches to the still-running browser.
import React, { useEffect, useState } from 'react'

import { listServerHosts, type ServerHost } from '../../runtime/server-host-client'
import { HostBrowserPane } from './HostBrowserPane'

type Session = { hostId: string; workdir: string; initialUrl?: string }

export default function HostBrowserView(): React.JSX.Element {
  const [hosts, setHosts] = useState<ServerHost[]>([])
  const [hostId, setHostId] = useState('')
  const [workdir, setWorkdir] = useState('')
  const [initialUrl, setInitialUrl] = useState('')
  const [session, setSession] = useState<Session | null>(null)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    let alive = true
    listServerHosts()
      .then((list) => {
        if (!alive) return
        const ssh = list.filter((h) => h.kind === 'ssh')
        setHosts(ssh)
        // Default to the first SSH host without clobbering a manual choice.
        setHostId((prev) => prev || (ssh[0]?.id ?? ''))
      })
      .catch((e: unknown) => {
        if (alive) setError(e instanceof Error ? e.message : String(e))
      })
    return () => {
      alive = false
    }
  }, [])

  if (session) {
    return (
      <div style={{ height: '100%', display: 'flex', flexDirection: 'column', minHeight: 0 }}>
        <button
          type="button"
          onClick={() => setSession(null)}
          style={{ margin: 8, alignSelf: 'flex-start' }}
        >
          ← Choose another host
        </button>
        <div style={{ flex: 1, minHeight: 0 }}>
          <HostBrowserPane
            hostId={session.hostId}
            workdir={session.workdir}
            initialUrl={session.initialUrl}
          />
        </div>
      </div>
    )
  }

  return (
    <div
      style={{
        padding: 24,
        maxWidth: 560,
        margin: '0 auto',
        display: 'flex',
        flexDirection: 'column',
        gap: 12
      }}
    >
      <h2 style={{ margin: 0 }}>Host Browser</h2>
      <p style={{ opacity: 0.7, fontSize: 13, margin: 0 }}>
        Run a headless Chromium on a remote host and watch it live here. The browser
        lives on the host, so it survives this Mac sleeping; reopening re-attaches.
      </p>
      {error ? <div style={{ color: 'var(--error, #c00)' }}>{error}</div> : null}
      <label style={{ display: 'flex', flexDirection: 'column', gap: 4 }}>
        Host
        <select value={hostId} onChange={(e) => setHostId(e.target.value)}>
          {hosts.length === 0 ? <option value="">No SSH hosts registered</option> : null}
          {hosts.map((h) => (
            <option key={h.id} value={h.id}>
              {h.name} ({h.hostname ?? h.id})
            </option>
          ))}
        </select>
      </label>
      <label style={{ display: 'flex', flexDirection: 'column', gap: 4 }}>
        Working dir on the host
        <input
          value={workdir}
          onChange={(e) => setWorkdir(e.target.value)}
          placeholder="/home/you/project"
          spellCheck={false}
        />
      </label>
      <label style={{ display: 'flex', flexDirection: 'column', gap: 4 }}>
        Initial URL (optional)
        <input
          value={initialUrl}
          onChange={(e) => setInitialUrl(e.target.value)}
          placeholder="http://localhost:3000"
          spellCheck={false}
        />
      </label>
      <button
        type="button"
        disabled={!hostId || !workdir.trim()}
        onClick={() =>
          setSession({
            hostId,
            workdir: workdir.trim(),
            initialUrl: initialUrl.trim() || undefined
          })
        }
      >
        Connect
      </button>
    </div>
  )
}
