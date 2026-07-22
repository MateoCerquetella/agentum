export function visibleOperationalProjectCount(args: {
  availableWidth: number
  projectWidths: readonly number[]
  reservedWidth?: number
}): number {
  let remaining = Math.max(0, args.availableWidth - (args.reservedWidth ?? 96))
  let count = 0
  for (const width of args.projectWidths) {
    const required = Math.max(48, width) + 6
    if (required > remaining) break
    remaining -= required
    count++
  }
  return count
}
