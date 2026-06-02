import { beforeEach, describe, expect, it, vi } from 'vitest'

const trackMock = vi.hoisted(() => vi.fn())

vi.mock('@/lib/telemetry', () => ({
  track: trackMock
}))

import {
  getAgentumCliFeatureTipTelemetrySource,
  trackAgentumCliFeatureTipSetupClicked,
  trackAgentumCliFeatureTipSetupResult,
  trackAgentumCliFeatureTipShown
} from './feature-tip-telemetry'

describe('feature tip telemetry', () => {
  beforeEach(() => {
    trackMock.mockClear()
  })

  it('keeps feature tip sources low-cardinality', () => {
    expect(getAgentumCliFeatureTipTelemetrySource('app_open')).toBe('app_open')
    expect(getAgentumCliFeatureTipTelemetrySource('settings')).toBe('manual')
    expect(getAgentumCliFeatureTipTelemetrySource(undefined)).toBe('manual')
  })

  it('tracks CLI tip exposure once per explicit call', () => {
    trackAgentumCliFeatureTipShown('app_open')

    expect(trackMock).toHaveBeenCalledTimes(1)
    expect(trackMock).toHaveBeenCalledWith('agentum_cli_feature_tip_shown', {
      source: 'app_open'
    })
  })

  it('tracks setup click and result without raw CLI details', () => {
    trackAgentumCliFeatureTipSetupClicked('app_open')
    trackAgentumCliFeatureTipSetupResult('app_open', 'installed')

    expect(trackMock).toHaveBeenCalledTimes(2)
    expect(trackMock).toHaveBeenNthCalledWith(1, 'agentum_cli_feature_tip_setup_clicked', {
      source: 'app_open'
    })
    expect(trackMock).toHaveBeenNthCalledWith(2, 'agentum_cli_feature_tip_setup_result', {
      source: 'app_open',
      result: 'installed'
    })
  })
})
