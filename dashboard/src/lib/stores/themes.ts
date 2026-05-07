/**
 * Theme registry. Each theme is a delta on top of the base tokens defined
 * in `lib/themes/_vars.css` — applying a theme writes every entry in
 * `vars` to `:root` via `style.setProperty`. Switching back to the
 * default theme writes the originals (kept here as `'default'`) so we
 * don't need to track which keys a previous theme touched.
 *
 * Themes are split into 'dark' and 'light' modes; the command palette
 * groups them under separate headings. Default is the in-design "Coral
 * Noir" palette so existing users see no change until they switch.
 */

export type ThemeMode = 'dark' | 'light';

export interface Theme {
  id: string;
  label: string;
  mode: ThemeMode;
  /** Hex shown next to the label in pickers. */
  swatch: string;
  /** CSS-variable overrides applied to `:root`. */
  vars: Record<string, string>;
}

/* --- shared ramps -------------------------------------------------- */
/* Keep the per-theme record short by deriving brand/semantic colors
   from a tiny set of "ramps" the theme picks. The picker only swaps
   surfaces + foreground + accents; tool-dot aliases stay mapped to
   their semantic siblings via the theme's chosen accent palette. */

const baseDarkSelectionFg = '#ffffff';
const baseLightSelectionFg = '#1c1f24';

/* --- themes -------------------------------------------------------- */

export const THEMES: Theme[] = [
  /* ======= Dark ======= */
  {
    id: 'default',
    label: 'Coral Noir',
    mode: 'dark',
    swatch: '#f36458',
    vars: {
      '--bg':            '#0b0b0b',
      '--bg-2':          '#000000',
      '--surface':       '#212121',
      '--surface-2':     '#353535',
      '--border':        '#212121',
      '--border-2':      '#353535',
      '--bg-chrome':     '#0d0d0d',
      '--bg-row-hover':  '#161616',
      '--bg-card-top':   '#1a1a1a',
      '--bg-card-body':  '#0a0a0a',
      '--bg-term':       '#050505',
      '--bg-tb-hover':   '#2c2c2c',
      '--fg':            '#ffffff',
      '--fg-2':          '#b9b9b9',
      '--fg-3':          '#797979',
      '--cta':           '#f36458',
      '--link':          '#0052ef',
      '--green':         '#19d600',
      '--amber':         '#ffb454',
      '--magenta':       '#f000ff',
      '--blu':           '#55beff',
      '--crash':         '#ff5555',
      '--selection-fg':  baseDarkSelectionFg,
      '--scanline':      'rgba(255, 255, 255, 0.012)'
    }
  },
  {
    id: 'dracula',
    label: 'Dracula',
    mode: 'dark',
    swatch: '#bd93f9',
    vars: {
      '--bg':            '#282a36',
      '--bg-2':          '#1e1f29',
      '--surface':       '#373948',
      '--surface-2':     '#44475a',
      '--border':        '#373948',
      '--border-2':      '#44475a',
      '--bg-chrome':     '#21222c',
      '--bg-row-hover':  '#2d2f3c',
      '--bg-card-top':   '#2a2c38',
      '--bg-card-body':  '#22232e',
      '--bg-term':       '#1b1c25',
      '--bg-tb-hover':   '#4a4d62',
      '--fg':            '#f8f8f2',
      '--fg-2':          '#c5c8d6',
      '--fg-3':          '#7e8194',
      '--cta':           '#ff79c6',
      '--link':          '#bd93f9',
      '--green':         '#50fa7b',
      '--amber':         '#ffb86c',
      '--magenta':       '#ff79c6',
      '--blu':           '#8be9fd',
      '--crash':         '#ff5555',
      '--selection-fg':  baseDarkSelectionFg,
      '--scanline':      'rgba(255, 255, 255, 0.012)'
    }
  },
  {
    id: 'tokyo-night',
    label: 'Tokyo Night',
    mode: 'dark',
    swatch: '#7aa2f7',
    vars: {
      '--bg':            '#1a1b26',
      '--bg-2':          '#13141c',
      '--surface':       '#24283b',
      '--surface-2':     '#2f3549',
      '--border':        '#24283b',
      '--border-2':      '#2f3549',
      '--bg-chrome':     '#16161e',
      '--bg-row-hover':  '#1f2233',
      '--bg-card-top':   '#1d1f2c',
      '--bg-card-body':  '#15161e',
      '--bg-term':       '#0d0e16',
      '--bg-tb-hover':   '#3a4060',
      '--fg':            '#c0caf5',
      '--fg-2':          '#a9b1d6',
      '--fg-3':          '#737aa2',
      '--cta':           '#f7768e',
      '--link':          '#7aa2f7',
      '--green':         '#9ece6a',
      '--amber':         '#e0af68',
      '--magenta':       '#bb9af7',
      '--blu':           '#7dcfff',
      '--crash':         '#f7768e',
      '--selection-fg':  baseDarkSelectionFg,
      '--scanline':      'rgba(255, 255, 255, 0.012)'
    }
  },
  {
    id: 'monokai',
    label: 'Monokai',
    mode: 'dark',
    swatch: '#f92672',
    vars: {
      '--bg':            '#272822',
      '--bg-2':          '#1d1e19',
      '--surface':       '#373831',
      '--surface-2':     '#49483e',
      '--border':        '#373831',
      '--border-2':      '#49483e',
      '--bg-chrome':     '#222319',
      '--bg-row-hover':  '#2f302a',
      '--bg-card-top':   '#2d2e25',
      '--bg-card-body':  '#22221c',
      '--bg-term':       '#1a1b15',
      '--bg-tb-hover':   '#52524a',
      '--fg':            '#f8f8f2',
      '--fg-2':          '#cbcbc2',
      '--fg-3':          '#75715e',
      '--cta':           '#f92672',
      '--link':          '#66d9ef',
      '--green':         '#a6e22e',
      '--amber':         '#fd971f',
      '--magenta':       '#ae81ff',
      '--blu':           '#66d9ef',
      '--crash':         '#f92672',
      '--selection-fg':  baseDarkSelectionFg,
      '--scanline':      'rgba(255, 255, 255, 0.012)'
    }
  },
  {
    id: 'nord',
    label: 'Nord',
    mode: 'dark',
    swatch: '#88c0d0',
    vars: {
      '--bg':            '#2e3440',
      '--bg-2':          '#242933',
      '--surface':       '#3b4252',
      '--surface-2':     '#434c5e',
      '--border':        '#3b4252',
      '--border-2':      '#4c566a',
      '--bg-chrome':     '#2a2f3a',
      '--bg-row-hover':  '#353b48',
      '--bg-card-top':   '#323844',
      '--bg-card-body':  '#272b35',
      '--bg-term':       '#1f232c',
      '--bg-tb-hover':   '#4c566a',
      '--fg':            '#eceff4',
      '--fg-2':          '#d8dee9',
      '--fg-3':          '#7b8597',
      '--cta':           '#bf616a',
      '--link':          '#88c0d0',
      '--green':         '#a3be8c',
      '--amber':         '#ebcb8b',
      '--magenta':       '#b48ead',
      '--blu':           '#81a1c1',
      '--crash':         '#bf616a',
      '--selection-fg':  baseDarkSelectionFg,
      '--scanline':      'rgba(255, 255, 255, 0.010)'
    }
  },
  {
    id: 'gruvbox-dark',
    label: 'Gruvbox Dark',
    mode: 'dark',
    swatch: '#fe8019',
    vars: {
      '--bg':            '#282828',
      '--bg-2':          '#1d2021',
      '--surface':       '#3c3836',
      '--surface-2':     '#504945',
      '--border':        '#3c3836',
      '--border-2':      '#504945',
      '--bg-chrome':     '#1d2021',
      '--bg-row-hover':  '#32302f',
      '--bg-card-top':   '#2d2c2a',
      '--bg-card-body':  '#22211e',
      '--bg-term':       '#181818',
      '--bg-tb-hover':   '#665c54',
      '--fg':            '#ebdbb2',
      '--fg-2':          '#d5c4a1',
      '--fg-3':          '#928374',
      '--cta':           '#fe8019',
      '--link':          '#83a598',
      '--green':         '#b8bb26',
      '--amber':         '#fabd2f',
      '--magenta':       '#d3869b',
      '--blu':           '#83a598',
      '--crash':         '#fb4934',
      '--selection-fg':  baseDarkSelectionFg,
      '--scanline':      'rgba(255, 255, 255, 0.012)'
    }
  },
  {
    id: 'one-dark',
    label: 'One Dark',
    mode: 'dark',
    swatch: '#61afef',
    vars: {
      '--bg':            '#282c34',
      '--bg-2':          '#1f2229',
      '--surface':       '#353b45',
      '--surface-2':     '#3e4451',
      '--border':        '#353b45',
      '--border-2':      '#4b5263',
      '--bg-chrome':     '#21252b',
      '--bg-row-hover':  '#2c313a',
      '--bg-card-top':   '#2a2f37',
      '--bg-card-body':  '#21252b',
      '--bg-term':       '#181a1f',
      '--bg-tb-hover':   '#4b5263',
      '--fg':            '#abb2bf',
      '--fg-2':          '#9da5b4',
      '--fg-3':          '#5c6370',
      '--cta':           '#e06c75',
      '--link':          '#61afef',
      '--green':         '#98c379',
      '--amber':         '#d19a66',
      '--magenta':       '#c678dd',
      '--blu':           '#56b6c2',
      '--crash':         '#e06c75',
      '--selection-fg':  baseDarkSelectionFg,
      '--scanline':      'rgba(255, 255, 255, 0.010)'
    }
  },
  {
    id: 'matrix',
    label: 'Matrix',
    mode: 'dark',
    swatch: '#00ff41',
    vars: {
      '--bg':            '#000800',
      '--bg-2':          '#000000',
      '--surface':       '#031a05',
      '--surface-2':     '#062a09',
      '--border':        '#062a09',
      '--border-2':      '#0c4011',
      '--bg-chrome':     '#020e03',
      '--bg-row-hover':  '#062a09',
      '--bg-card-top':   '#031a05',
      '--bg-card-body':  '#000600',
      '--bg-term':       '#000200',
      '--bg-tb-hover':   '#0c4011',
      '--fg':            '#39ff5b',
      '--fg-2':          '#1fbb38',
      '--fg-3':          '#0f7f25',
      '--cta':           '#00ff41',
      '--link':          '#39ff5b',
      '--green':         '#00ff41',
      '--amber':         '#7fff00',
      '--magenta':       '#39ff5b',
      '--blu':           '#39ff5b',
      '--crash':         '#ff3030',
      '--selection-fg':  '#000800',
      '--scanline':      'rgba(0, 255, 65, 0.05)'
    }
  },
  {
    id: 'retro',
    label: 'Retro CRT',
    mode: 'dark',
    swatch: '#ffb000',
    vars: {
      '--bg':            '#1a0e00',
      '--bg-2':          '#0d0700',
      '--surface':       '#2a1a05',
      '--surface-2':     '#3d2a0a',
      '--border':        '#3d2a0a',
      '--border-2':      '#5a3e10',
      '--bg-chrome':     '#1f1100',
      '--bg-row-hover':  '#2a1a05',
      '--bg-card-top':   '#241500',
      '--bg-card-body':  '#160a00',
      '--bg-term':       '#0a0500',
      '--bg-tb-hover':   '#5a3e10',
      '--fg':            '#ffb000',
      '--fg-2':          '#cc8800',
      '--fg-3':          '#7a5200',
      '--cta':           '#ffb000',
      '--link':          '#ffd166',
      '--green':         '#a8d600',
      '--amber':         '#ffb000',
      '--magenta':       '#ff7d00',
      '--blu':           '#ffd166',
      '--crash':         '#ff5050',
      '--selection-fg':  '#1a0e00',
      '--scanline':      'rgba(255, 176, 0, 0.06)'
    }
  },
  {
    id: 'synthwave',
    label: 'Synthwave',
    mode: 'dark',
    swatch: '#ff7edb',
    vars: {
      '--bg':            '#1b1238',
      '--bg-2':          '#120a26',
      '--surface':       '#2a1d52',
      '--surface-2':     '#3a2a72',
      '--border':        '#2a1d52',
      '--border-2':      '#4a3792',
      '--bg-chrome':     '#170e30',
      '--bg-row-hover':  '#251846',
      '--bg-card-top':   '#221542',
      '--bg-card-body':  '#150a2c',
      '--bg-term':       '#0d0620',
      '--bg-tb-hover':   '#4a3792',
      '--fg':            '#f5e1ff',
      '--fg-2':          '#c1a5e8',
      '--fg-3':          '#7d6ba8',
      '--cta':           '#ff7edb',
      '--link':          '#36f9f6',
      '--green':         '#72f1b8',
      '--amber':         '#fede5d',
      '--magenta':       '#ff7edb',
      '--blu':           '#36f9f6',
      '--crash':         '#fe4450',
      '--selection-fg':  '#1b1238',
      '--scanline':      'rgba(255, 126, 219, 0.04)'
    }
  },

  /* ======= Light ======= */
  {
    id: 'github-light',
    label: 'GitHub Light',
    mode: 'light',
    swatch: '#0969da',
    vars: {
      '--bg':            '#ffffff',
      '--bg-2':          '#f6f8fa',
      '--surface':       '#f6f8fa',
      '--surface-2':     '#eaeef2',
      '--border':        '#d0d7de',
      '--border-2':      '#afb8c1',
      '--bg-chrome':     '#f6f8fa',
      '--bg-row-hover':  '#eaeef2',
      '--bg-card-top':   '#f6f8fa',
      '--bg-card-body':  '#ffffff',
      '--bg-term':       '#f6f8fa',
      '--bg-tb-hover':   '#d0d7de',
      '--fg':            '#1f2328',
      '--fg-2':          '#424a53',
      '--fg-3':          '#656d76',
      '--cta':           '#cf222e',
      '--link':          '#0969da',
      '--green':         '#1a7f37',
      '--amber':         '#9a6700',
      '--magenta':       '#8250df',
      '--blu':           '#0969da',
      '--crash':         '#cf222e',
      '--selection-fg':  '#ffffff',
      '--scanline':      'transparent'
    }
  },
  {
    id: 'solarized-light',
    label: 'Solarized Light',
    mode: 'light',
    swatch: '#268bd2',
    vars: {
      '--bg':            '#fdf6e3',
      '--bg-2':          '#f3ecd3',
      '--surface':       '#eee8d5',
      '--surface-2':     '#e3dcc4',
      '--border':        '#e3dcc4',
      '--border-2':      '#c4bda0',
      '--bg-chrome':     '#f5eed4',
      '--bg-row-hover':  '#e8e1c8',
      '--bg-card-top':   '#eee8d5',
      '--bg-card-body':  '#fdf6e3',
      '--bg-term':       '#f5eed4',
      '--bg-tb-hover':   '#c4bda0',
      '--fg':            '#073642',
      '--fg-2':          '#586e75',
      '--fg-3':          '#93a1a1',
      '--cta':           '#dc322f',
      '--link':          '#268bd2',
      '--green':         '#859900',
      '--amber':         '#b58900',
      '--magenta':       '#d33682',
      '--blu':           '#2aa198',
      '--crash':         '#dc322f',
      '--selection-fg':  '#fdf6e3',
      '--scanline':      'transparent'
    }
  },
  {
    id: 'gruvbox-light',
    label: 'Gruvbox Light',
    mode: 'light',
    swatch: '#af3a03',
    vars: {
      '--bg':            '#fbf1c7',
      '--bg-2':          '#f2e5bc',
      '--surface':       '#ebdbb2',
      '--surface-2':     '#d5c4a1',
      '--border':        '#d5c4a1',
      '--border-2':      '#bdae93',
      '--bg-chrome':     '#f2e5bc',
      '--bg-row-hover':  '#ebdbb2',
      '--bg-card-top':   '#ebdbb2',
      '--bg-card-body':  '#fbf1c7',
      '--bg-term':       '#f2e5bc',
      '--bg-tb-hover':   '#bdae93',
      '--fg':            '#3c3836',
      '--fg-2':          '#504945',
      '--fg-3':          '#7c6f64',
      '--cta':           '#af3a03',
      '--link':          '#076678',
      '--green':         '#79740e',
      '--amber':         '#b57614',
      '--magenta':       '#8f3f71',
      '--blu':           '#076678',
      '--crash':         '#9d0006',
      '--selection-fg':  '#fbf1c7',
      '--scanline':      'transparent'
    }
  },
  {
    id: 'one-light',
    label: 'One Light',
    mode: 'light',
    swatch: '#4078f2',
    vars: {
      '--bg':            '#fafafa',
      '--bg-2':          '#f0f0f0',
      '--surface':       '#eaeaeb',
      '--surface-2':     '#dcdcdd',
      '--border':        '#dcdcdd',
      '--border-2':      '#c8c8c9',
      '--bg-chrome':     '#f0f0f0',
      '--bg-row-hover':  '#e5e5e6',
      '--bg-card-top':   '#eaeaeb',
      '--bg-card-body':  '#fafafa',
      '--bg-term':       '#f0f0f0',
      '--bg-tb-hover':   '#c8c8c9',
      '--fg':            '#383a42',
      '--fg-2':          '#5c6370',
      '--fg-3':          '#a0a1a7',
      '--cta':           '#e45649',
      '--link':          '#4078f2',
      '--green':         '#50a14f',
      '--amber':         '#c18401',
      '--magenta':       '#a626a4',
      '--blu':           '#0184bc',
      '--crash':         '#e45649',
      '--selection-fg':  '#ffffff',
      '--scanline':      'transparent'
    }
  }
];

const DEFAULT_THEME_ID = 'default';

export function findTheme(id: string): Theme {
  return THEMES.find((t) => t.id === id) ?? THEMES[0];
}

export function getDefaultThemeId(): string {
  return DEFAULT_THEME_ID;
}

export function applyThemeVars(t: Theme): void {
  if (typeof document === 'undefined') return;
  const root = document.documentElement;
  for (const [k, v] of Object.entries(t.vars)) {
    root.style.setProperty(k, v);
  }
  // The theme owns the swatch-equivalent accent. The user's accent
  // tweak (--cta) is re-applied after this call by `applyTweaks` so
  // a chosen accent always wins.
  root.dataset.theme = t.id;
  root.dataset.themeMode = t.mode;
}
