// Low-level authed HTTP against the embedded agentum-server, shared by the
// per-domain server clients (git, fs, …). Loopback + no-auth today, but
// token-ready via server-endpoint.ts.
import { apiUrl, getServerEndpoint } from './server-endpoint'

async function authHeaders(): Promise<Record<string, string>> {
  const { token } = await getServerEndpoint()
  return token ? { Authorization: `Bearer ${token}` } : {}
}

async function run(path: string, init?: RequestInit): Promise<Response> {
  const url = await apiUrl(path)
  const res = await fetch(url, {
    ...init,
    headers: {
      ...(init?.body ? { 'Content-Type': 'application/json' } : {}),
      ...(await authHeaders()),
      ...(init?.headers ?? {})
    }
  })
  if (!res.ok) {
    const detail = await res.text().catch(() => '')
    throw new Error(`agentum-server ${res.status} on ${path}${detail ? ` — ${detail}` : ''}`)
  }
  return res
}

export async function getJson<T>(path: string): Promise<T> {
  return (await run(path)).json() as Promise<T>
}

export async function getText(path: string): Promise<string> {
  return (await run(path)).text()
}

async function bodyJson<T>(res: Response): Promise<T> {
  const text = await res.text()
  return (text ? JSON.parse(text) : undefined) as T
}

export async function postJson<T>(path: string, body?: unknown): Promise<T> {
  return bodyJson<T>(
    await run(path, {
      method: 'POST',
      body: body === undefined ? undefined : JSON.stringify(body)
    })
  )
}

export async function patchJson<T>(path: string, body: unknown): Promise<T> {
  return bodyJson<T>(await run(path, { method: 'PATCH', body: JSON.stringify(body) }))
}

export async function putJson<T>(path: string, body: unknown): Promise<T> {
  return bodyJson<T>(await run(path, { method: 'PUT', body: JSON.stringify(body) }))
}

export async function del(path: string): Promise<void> {
  await run(path, { method: 'DELETE' })
}

/** Build a `?a=1&b=2` query string, skipping `undefined` values. */
export function qs(params: Record<string, string | number | boolean | undefined>): string {
  const sp = new URLSearchParams()
  for (const [key, value] of Object.entries(params)) {
    if (value !== undefined) {
      sp.set(key, String(value))
    }
  }
  const s = sp.toString()
  return s ? `?${s}` : ''
}
