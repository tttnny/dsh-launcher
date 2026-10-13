// The app's icon set: one stroke style everywhere (16px grid, 1.5 stroke,
// round joins). Views that used to carry inline SVGs at three different
// weights now all read from here, so the icon language cannot drift.

export type IconName =
  | 'instances'
  | 'library'
  | 'settings'
  | 'health'
  | 'external'
  | 'copy'
  | 'plus'
  | 'trash'
  | 'folder'
  | 'terminal'
  | 'log'
  | 'restart'
  | 'stop'
  | 'play'
  | 'chevron-down'
  | 'chevron-right'
  | 'back'
  | 'sidebar'
  | 'search'
  | 'check'
  | 'close'
  | 'download'
  | 'link'
  | 'refresh'
  | 'alert'
  | 'box'
  | 'profile'
  | 'plugin'
  | 'bundle'

/** Filled glyphs opt out of the stroke treatment. */
export const FILLED: Partial<Record<IconName, string>> = {
  play: '<path d="M5.6 3.4 12.6 8l-7 4.6z"/>',
  stop: '<rect x="4.5" y="4.5" width="7" height="7" rx="1.6"/>',
}

export const PATHS: Record<IconName, string> = {
  instances:
    '<rect x="1.75" y="2.75" width="12.5" height="10.5" rx="2.5"/><path d="M4.75 6.25h6.5M4.75 9.75h4"/>',
  library:
    '<path d="M1.75 5.5 8 2.25l6.25 3.25v5L8 13.75 1.75 10.5z"/><path d="M1.75 5.5 8 8.75l6.25-3.25M8 8.75v5"/>',
  settings:
    '<path d="M2.5 5h11M2.5 11h11"/><circle cx="6" cy="5" r="1.75"/><circle cx="10.5" cy="11" r="1.75"/>',
  health: '<path d="M1.75 8.25h2.75l1.5-3.5 2.25 7 1.5-3.5h4.5"/>',
  external:
    '<path d="M6.75 3.5H3.5a1.25 1.25 0 0 0-1.25 1.25v7.5A1.25 1.25 0 0 0 3.5 13.5h7.5a1.25 1.25 0 0 0 1.25-1.25V9"/><path d="M9.75 2.5h3.75v3.75M13.3 2.7 7.6 8.4"/>',
  copy:
    '<rect x="5.75" y="5.75" width="7.5" height="7.5" rx="1.75"/><path d="M10.25 5.75v-2A1.5 1.5 0 0 0 8.75 2.25H4A1.5 1.5 0 0 0 2.5 3.75V9A1.5 1.5 0 0 0 4 10.5h1.75"/>',
  plus: '<path d="M8 3.5v9M3.5 8h9"/>',
  trash:
    '<path d="M2.5 4.5h11M6 4.5V3.4a.9.9 0 0 1 .9-.9h2.2a.9.9 0 0 1 .9.9V4.5"/><path d="M4.1 4.5l.55 8.1a1 1 0 0 0 1 .9h4.7a1 1 0 0 0 1-.9l.55-8.1"/>',
  folder:
    '<path d="M1.75 4.5A1.75 1.75 0 0 1 3.5 2.75h2.35l1.6 1.75h4.95A1.75 1.75 0 0 1 14.15 6.25v5A1.75 1.75 0 0 1 12.4 13H3.5a1.75 1.75 0 0 1-1.75-1.75z"/>',
  terminal:
    '<rect x="1.75" y="2.75" width="12.5" height="10.5" rx="2.2"/><path d="M4.6 6.4 6.6 8.25l-2 1.85M8.6 9.9h3"/>',
  log: '<path d="M9.75 1.75H4.5A1.25 1.25 0 0 0 3.25 3v10a1.25 1.25 0 0 0 1.25 1.25h7A1.25 1.25 0 0 0 12.75 13V4.75z"/><path d="M9.75 1.75V4.75h3M5.9 8.6h4.2M5.9 11h2.6"/>',
  restart:
    '<path d="M2.9 6.4A5.25 5.25 0 1 1 2.75 8"/><path d="M2.5 2.75v3.5h3.5"/><path d="M13.1 9.6a5.25 5.25 0 0 1-5.1 3.65"/>',
  stop: '',
  play: '',
  'chevron-down': '<path d="M4 6.4 8 10.4l4-4"/>',
  'chevron-right': '<path d="M6 4l4 4-4 4"/>',
  back: '<path d="M9.6 3.4 5 8l4.6 4.6"/>',
  sidebar:
    '<rect x="1.75" y="3" width="12.5" height="10" rx="2.5"/><path d="M6.1 3v10"/>',
  search: '<circle cx="7" cy="7" r="4.25"/><path d="M10.2 10.2 14 14"/>',
  check: '<path d="M3.5 8.4 6.4 11.3 12.5 5.2"/>',
  close: '<path d="M4 4l8 8M12 4l-8 8"/>',
  download: '<path d="M8 2.5v7.25M4.9 7.15 8 10.25l3.1-3.1M2.9 13.25h10.2"/>',
  link: '<path d="M6.6 9.4a2.6 2.6 0 0 0 3.7 0l2.1-2.1a2.62 2.62 0 0 0-3.7-3.7l-1 1"/><path d="M9.4 6.6a2.6 2.6 0 0 0-3.7 0L3.6 8.7a2.62 2.62 0 0 0 3.7 3.7l1-1"/>',
  refresh:
    '<path d="M13.25 8A5.25 5.25 0 1 1 8 2.75c1.7 0 3.2.8 4.15 2.05"/><path d="M13.4 2.6v3.3h-3.3"/>',
  alert: '<path d="M8 2.75 14.2 13H1.8z"/><path d="M8 6.5v3.1M8 11.4h.01"/>',
  box: '<rect x="2.25" y="2.75" width="11.5" height="10.5" rx="2"/><path d="M2.25 6.5h11.5M6.4 9.6h3.2"/>',
  profile:
    '<circle cx="8" cy="5.75" r="2.75"/><path d="M2.9 13.4a5.4 5.4 0 0 1 10.2 0"/>',
  plugin:
    '<path d="M6.4 2.6a1.6 1.6 0 0 1 3.2 0v1.1h2.4a.9.9 0 0 1 .9.9v2.4h1.1a1.6 1.6 0 0 1 0 3.2h-1.1v2.4a.9.9 0 0 1-.9.9H9.6v-1.1a1.6 1.6 0 0 0-3.2 0v1.1H4a.9.9 0 0 1-.9-.9V9.6H2a1.6 1.6 0 0 1 0-3.2h1.1V4.6a.9.9 0 0 1 .9-.9h2.4z"/>',
  bundle:
    '<rect x="2.25" y="4.5" width="11.5" height="8.75" rx="1.75"/><path d="M2.25 7.5h11.5M6.4 7.5v5.75"/>',
}
