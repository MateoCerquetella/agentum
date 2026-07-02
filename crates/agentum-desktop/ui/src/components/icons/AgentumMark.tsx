import React from 'react'

// Why: the agentum brand mark (the stacked-square "A" pyramid, mirroring
// `resources/logo.svg`) is the app's identity glyph. The source logo is a grey
// top→bottom gradient that disappears on a dark surface, so this React version
// paints every cell in `currentColor` — letting each caller (the Chat assistant
// avatar, future nav/brand surfaces) size and colour it to fit, e.g. white on a
// gradient tile. The rect geometry is kept in sync with `resources/logo.svg`; the
// viewBox is cropped to the glyph's bounds (the logo's is padded) so the mark
// fills the icon box like any other icon.
export function AgentumMark({ className }: { className?: string }): React.JSX.Element {
  return (
    <svg viewBox="22 23 56 53" aria-hidden className={className} fill="currentColor">
      <g transform="translate(0 4)">
        {/* apex */}
        <rect x="44" y="22" width="12" height="12" rx="2.4" />
        {/* descending paired rows form the "A" silhouette */}
        <rect x="39.28" y="32.28" width="10.44" height="10.44" rx="2.09" />
        <rect x="50.28" y="32.28" width="10.44" height="10.44" rx="2.09" />
        <rect x="34.56" y="42.56" width="8.88" height="8.88" rx="1.78" />
        <rect x="56.56" y="42.56" width="8.88" height="8.88" rx="1.78" />
        <rect x="29.84" y="52.84" width="7.32" height="7.32" rx="1.46" />
        <rect x="62.84" y="52.84" width="7.32" height="7.32" rx="1.46" />
        <rect x="25.12" y="63.12" width="5.76" height="5.76" rx="1.15" />
        <rect x="69.12" y="63.12" width="5.76" height="5.76" rx="1.15" />
      </g>
    </svg>
  )
}
