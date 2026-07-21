import { describe, expect, it } from 'vitest'
import { isHostUnreachableFsError } from './file-explorer-error'

describe('isHostUnreachableFsError', () => {
  it('classifies the full server error for an SSH connect timeout', () => {
    expect(
      isHostUnreachableFsError(
        'agentum-server 400 on /api/fs/entries?path=%2Fhome%2Fuser%2Frepo&show_hidden=true&host_id=d23713ad-be98-4c8a-b1b9-b5809515c88a — {"error":"remote fs: ssh/tmux exited with status Some(255) (stderr: ssh: connect to host 100.85.185.109 port 22: Operation timed out)"}'
      )
    ).toBe(true)
  })

  it('classifies common ssh connect-level failures in a remote-fs context', () => {
    for (const stderr of [
      'ssh: connect to host 10.0.0.5 port 22: Connection refused',
      'ssh: connect to host example.dev port 22: No route to host',
      'ssh: Could not resolve hostname example.dev: Name or service not known',
      'kex_exchange_identification: read: Connection reset by peer',
      'Connection closed by remote host'
    ]) {
      expect(
        isHostUnreachableFsError(
          `remote fs: ssh/tmux exited with status Some(255) (stderr: ${stderr})`
        )
      ).toBe(true)
    }
  })

  it('classifies the server-side ssh deadline', () => {
    expect(isHostUnreachableFsError('remote fs: ssh command timed out')).toBe(true)
  })

  it('does not classify ssh auth failures — their message is actionable', () => {
    expect(
      isHostUnreachableFsError(
        'remote fs: ssh/tmux exited with status Some(255) (stderr: user@10.0.0.5: Permission denied (publickey).)'
      )
    ).toBe(false)
  })

  it('does not classify remote fs errors from a reachable host', () => {
    expect(
      isHostUnreachableFsError(
        'remote fs: ssh/tmux exited with status Some(1) (stderr: find: /gone: No such file or directory)'
      )
    ).toBe(false)
  })

  it('does not classify local errors that mention connection words without ssh context', () => {
    expect(isHostUnreachableFsError('proxy error: connection refused')).toBe(false)
    expect(isHostUnreachableFsError('EACCES: permission denied, scandir /root')).toBe(false)
  })
})
