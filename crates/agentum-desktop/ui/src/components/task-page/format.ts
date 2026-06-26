// Why: Intl.RelativeTimeFormat allocation is non-trivial, and previously we
// built a new formatter per work-item row render. Hoisting to module scope
// means all rows share one instance — zero per-row allocation cost.
const relativeTimeFormatter = new Intl.RelativeTimeFormat(undefined, { numeric: 'auto' })

export function formatRelativeTime(input: string): string {
  const date = new Date(input)
  if (Number.isNaN(date.getTime())) {
    return 'recently'
  }

  const diffMs = date.getTime() - Date.now()
  const diffMinutes = Math.round(diffMs / 60_000)

  if (Math.abs(diffMinutes) < 60) {
    return relativeTimeFormatter.format(diffMinutes, 'minute')
  }

  const diffHours = Math.round(diffMinutes / 60)
  if (Math.abs(diffHours) < 24) {
    return relativeTimeFormatter.format(diffHours, 'hour')
  }

  const diffDays = Math.round(diffHours / 24)
  return relativeTimeFormatter.format(diffDays, 'day')
}
