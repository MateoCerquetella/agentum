import React from 'react'
import { getFileTypeIcon } from '@/lib/file-type-icons'

// Thin adapter for the file explorer rows: resolve the file-type icon (colored
// extension badge for recognised types, Lucide glyph otherwise) and render it
// at the explorer's default size. The badge styling/colors live in main.css
// (`.file-type-badge`); the Lucide fallback honors the muted text color here.
export function FileTypeBadge({
  name,
  className
}: {
  name: string
  className?: string
}): React.JSX.Element {
  const Icon = getFileTypeIcon(name)
  return <Icon className={className ?? 'size-3.5 shrink-0 text-muted-foreground'} />
}
