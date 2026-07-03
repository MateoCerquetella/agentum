import { describe, expect, it } from 'vitest'
import {
  FEATURE_TIPS,
  getCompletedFeatureTipIds,
  getOrderedUnseenFeatureTips,
  normalizeFeatureTipIds,
  type FeatureTipId
} from './feature-tips'

describe('feature tips', () => {
  it('surfaces unseen tips', () => {
    const tips = getOrderedUnseenFeatureTips({ seenTipIds: new Set<FeatureTipId>() })

    expect(tips.map((tip) => tip.id)).toEqual(['voice-dictation'])
  })

  it('skips tips the user has already seen', () => {
    const tips = getOrderedUnseenFeatureTips({
      seenTipIds: new Set<FeatureTipId>(['voice-dictation'])
    })

    expect(tips.map((tip) => tip.id)).toEqual([])
  })

  it('skips tips for features the user has already completed', () => {
    const tips = getOrderedUnseenFeatureTips({
      seenTipIds: new Set<FeatureTipId>(),
      completedTipIds: getCompletedFeatureTipIds({
        voiceDictationEnabled: true
      })
    })

    expect(tips.map((tip) => tip.id)).toEqual([])
  })

  it('skips tips for features the user has already interacted with', () => {
    const tips = getOrderedUnseenFeatureTips({
      seenTipIds: new Set<FeatureTipId>(),
      completedTipIds: getCompletedFeatureTipIds({
        voiceDictationEnabled: false,
        featureInteractions: {
          'voice-dictation': { firstInteractedAt: 100, interactionCount: 1 }
        }
      })
    })

    expect(tips.map((tip) => tip.id)).toEqual([])
  })

  it('normalizes persisted tip ids (dropping the removed agentum-cli tip)', () => {
    expect(
      normalizeFeatureTipIds(['feature-tour', 'agentum-cli', 'bogus', 'voice-dictation'])
    ).toEqual(['voice-dictation'])
  })

  it('does not label the voice dictation tip as new', () => {
    const voiceTip = FEATURE_TIPS.find((tip) => tip.id === 'voice-dictation')

    expect(voiceTip?.eyebrow).toBe('Tip')
    expect(voiceTip?.priority).toBe('unseen')
  })
})
