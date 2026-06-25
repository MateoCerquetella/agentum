import { describe, expect, it } from 'vitest'
import { MISSION_CONTROL_SOON_CARDS } from './mission-control-soon-cards'

describe('MISSION_CONTROL_SOON_CARDS', () => {
  it('lists exactly three coming-soon capabilities', () => {
    expect(MISSION_CONTROL_SOON_CARDS).toHaveLength(3)
  })

  it('leads with Agent Orchestration', () => {
    expect(MISSION_CONTROL_SOON_CARDS[0]).toMatchObject({
      id: 'agent-orchestration',
      title: 'Agent Orchestration',
      icon: 'orchestration'
    })
  })

  it('has unique ids, a known icon, and non-empty copy per card', () => {
    const ids = MISSION_CONTROL_SOON_CARDS.map((c) => c.id)
    expect(new Set(ids).size).toBe(ids.length)
    for (const card of MISSION_CONTROL_SOON_CARDS) {
      expect(['orchestration', 'schedule', 'cost']).toContain(card.icon)
      expect(card.title.length).toBeGreaterThan(0)
      expect(card.description.length).toBeGreaterThan(0)
    }
  })
})
