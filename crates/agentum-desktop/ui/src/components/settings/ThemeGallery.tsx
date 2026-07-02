import type React from 'react'
import { Check } from 'lucide-react'
import { cn } from '@/lib/utils'
import type { GlobalSettings } from '../../../../shared/types'

// VS Code-style appearance gallery: pick the window theme from labelled preview
// cards instead of a plain segmented control. Each card paints a tiny mock
// window in that theme's palette so the choice is visual. System shows both
// halves to signal "follows the OS".

type AppearanceValue = GlobalSettings['theme']

type Palette = {
  bg: string
  panel: string
  text: string
  accent: string
  border: string
}

// Representative colours pulled from the two document palettes in main.css
// (`:root` = light "Paper", `.dark` = "command center"). Hard-coded because the
// live CSS variables only resolve to the *current* theme, so a light preview
// could not read the dark values (or vice-versa) at runtime.
const LIGHT_PALETTE: Palette = {
  bg: '#ffffff',
  panel: '#f4f4f5',
  text: '#3f3f46',
  accent: '#e85544',
  border: '#e4e4e7'
}
const DARK_PALETTE: Palette = {
  bg: '#161619',
  panel: '#202027',
  text: '#d4d4d8',
  accent: '#f36458',
  border: '#2a2a30'
}

function MockWindow({ palette }: { palette: Palette }): React.JSX.Element {
  return (
    <div className="flex h-full w-full" style={{ backgroundColor: palette.bg }}>
      {/* sidebar rail */}
      <div
        className="flex h-full w-1/4 flex-col gap-1 p-1.5"
        style={{ backgroundColor: palette.panel, borderRight: `1px solid ${palette.border}` }}
      >
        <div className="h-1 w-full rounded-full" style={{ backgroundColor: palette.accent }} />
        <div className="h-1 w-3/4 rounded-full" style={{ backgroundColor: palette.text, opacity: 0.35 }} />
        <div className="h-1 w-2/3 rounded-full" style={{ backgroundColor: palette.text, opacity: 0.25 }} />
      </div>
      {/* content */}
      <div className="flex h-full flex-1 flex-col gap-1.5 p-2">
        <div className="h-1.5 w-1/2 rounded-full" style={{ backgroundColor: palette.text, opacity: 0.7 }} />
        <div className="h-1 w-full rounded-full" style={{ backgroundColor: palette.text, opacity: 0.3 }} />
        <div className="h-1 w-5/6 rounded-full" style={{ backgroundColor: palette.text, opacity: 0.3 }} />
        <div className="mt-auto h-2 w-1/3 rounded" style={{ backgroundColor: palette.accent }} />
      </div>
    </div>
  )
}

function PreviewSurface({ value }: { value: AppearanceValue }): React.JSX.Element {
  if (value === 'light') {
    return <MockWindow palette={LIGHT_PALETTE} />
  }
  if (value === 'dark') {
    return <MockWindow palette={DARK_PALETTE} />
  }
  // System — split the card so both palettes are visible at once.
  return (
    <div className="flex h-full w-full">
      <div className="h-full w-1/2 overflow-hidden">
        <MockWindow palette={LIGHT_PALETTE} />
      </div>
      <div className="h-full w-1/2 overflow-hidden">
        <MockWindow palette={DARK_PALETTE} />
      </div>
    </div>
  )
}

const OPTIONS: readonly { value: AppearanceValue; label: string }[] = [
  { value: 'system', label: 'System' },
  { value: 'light', label: 'Light' },
  { value: 'dark', label: 'Dark' }
]

export function ThemeGallery({
  value,
  onSelect
}: {
  value: AppearanceValue
  onSelect: (value: AppearanceValue) => void
}): React.JSX.Element {
  return (
    <div
      role="radiogroup"
      aria-label="App appearance"
      className="grid grid-cols-3 gap-2"
    >
      {OPTIONS.map((option) => {
        const selected = value === option.value
        return (
          <button
            key={option.value}
            type="button"
            role="radio"
            aria-checked={selected}
            onClick={() => onSelect(option.value)}
            className={cn(
              'group relative flex flex-col overflow-hidden rounded-lg border text-left transition-colors',
              selected
                ? 'border-primary ring-1 ring-primary'
                : 'border-border hover:border-primary/50'
            )}
          >
            <div className="h-16 w-full overflow-hidden">
              <PreviewSurface value={option.value} />
            </div>
            <div className="flex items-center justify-between gap-1 px-2 py-1.5">
              <span className="text-[12px] font-medium">{option.label}</span>
              {selected ? (
                <Check className="size-3.5 shrink-0 text-primary" aria-hidden />
              ) : null}
            </div>
          </button>
        )
      })}
    </div>
  )
}
