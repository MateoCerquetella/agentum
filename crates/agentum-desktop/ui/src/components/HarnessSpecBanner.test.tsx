// Spec 015 f2 — markup pins for the harness-offer banner, rendered via
// renderToStaticMarkup (the TabGroupPanel.sdd-bar.test.tsx pattern; no jsdom).
// The store is mocked module-level; the component modules are imported
// dynamically inside the tests so the mock factory never hits the TDZ.
import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it, vi } from 'vitest'

const STORE_STATE = {
  harnessOfferByWorktreeId: {
    'wt-1': {
      worktreeId: 'wt-1',
      workdir: '/workspace/feature',
      harnessDir: '.agentum-harness'
    }
  },
  clearWorkspaceHarnessOffer: () => {}
}

vi.mock('@/store', () => ({
  useAppStore: (selector: (s: typeof STORE_STATE) => unknown) => selector(STORE_STATE)
}))

async function importBanner(): Promise<typeof import('./HarnessSpecBanner')> {
  return await import('./HarnessSpecBanner')
}

describe('HarnessSpecBanner (host)', () => {
  it('renders the offer strip for a worktree with an offer', async () => {
    const { default: HarnessSpecBanner } = await importBanner()
    const html = renderToStaticMarkup(<HarnessSpecBanner worktreeId="wt-1" />)
    expect(html).toContain('Start Harness run')
    expect(html).toContain('.agentum-harness/feature_list.json')
    expect(html).toContain('Dismiss harness offer')
    // Load-bearing vs the launcher overlay's z-20: the strip must paint above.
    expect(html).toContain('z-30')
  })

  it('renders nothing when the worktree has no offer', async () => {
    const { default: HarnessSpecBanner } = await importBanner()
    expect(renderToStaticMarkup(<HarnessSpecBanner worktreeId="wt-other" />)).toBe('')
  })
})

describe('HarnessSpecBannerView', () => {
  it('busy disables both accept and dismiss', async () => {
    const { HarnessSpecBannerView } = await importBanner()
    const html = renderToStaticMarkup(
      <HarnessSpecBannerView
        harnessDir=".harness"
        busy={true}
        onAccept={() => {}}
        onDismiss={() => {}}
      />
    )
    // Two disabled="" attributes — the button classnames also contain the
    // `disabled:` Tailwind variant, so match the attribute form exactly.
    expect(html.match(/disabled=""/g)).toHaveLength(2)
  })

  it('idle disables nothing and shows the legacy dir name it was given', async () => {
    const { HarnessSpecBannerView } = await importBanner()
    const html = renderToStaticMarkup(
      <HarnessSpecBannerView
        harnessDir=".harness"
        busy={false}
        onAccept={() => {}}
        onDismiss={() => {}}
      />
    )
    expect(html.match(/disabled=""/g)).toBeNull()
    expect(html).toContain('.harness/feature_list.json')
  })
})
