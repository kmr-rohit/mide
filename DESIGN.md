# Design notes (from Sublime Text + Sublime Merge reference)

Source screenshots: `reference/sublime-1..4.jpg`. Colors below were sampled from
those images (JPEG, so ±2 per channel).

## Overall look

Light chrome around a dark editor: light sidebar and palette, grey tab bar, dark
editor/terminal body (Mariana-style scheme). Dense, small type, no icons-as-buttons
clutter, no activity bar, no breadcrumbs. Native macOS title bar.

## Layout (sublime-1, sublime-3)

```
┌ native title bar:  "<file> — <project folder>"                 ┐
├──────────┬───────────────────────────────────────┬────────────┤
│ FOLDERS  │ ◀▶ [ tab ][ tab ]                  + ▾│            │
│ tree     ├───────────────────────────────┬───────┤            │
│ (light)  │ gutter │ code                 │minimap│            │
│          │        │                      │       │            │
│          ├───────────────────────────────┴───────┤            │
│          │ terminal pane (same dark bg, splits up)            │
├──────────┴───────────────────────────────────────┴────────────┤
│ status: git branch, index ✓, working ✓, Ln/Col … branch Spaces Syntax │
└───────────────────────────────────────────────────────────────┘
```

- Sidebar: "FOLDERS" small-caps header, disclosure triangles, folder + file icons,
  ~17px row height, selected row is a darker grey fill, horizontal scrollbar at the
  bottom when names overflow.
- Tab bar: back/forward arrows at left, `+` and dropdown at right. Active tab is
  light with italic title (preview tab), close `x` on the tab.
- Editor: line-number gutter (current line number bold + highlighted), thin
  overlay scrollbar, minimap on the right edge, caret is a thin line, matching
  bracket highlighted with an outline.
- Terminal: a bottom pane that splits the editor area (not a separate panel
  chrome). Same dark background as the editor, no visible header.
- Status bar: single light-grey row, ~12px. Left: git branch, index/working
  state, `Line N, Column M`. Right: branch, `Spaces: N`, syntax name.

## Command palette (sublime-2)

Top-center overlay, ~470px wide, light background, white input row, result rows
with the command name left and the shortcut right-aligned in a muted color.
Selected row is a darker grey. Same widget serves Cmd+P (files), Cmd+Shift+P
(commands), Cmd+R (symbols), Ctrl+G (goto line).

## Git view (Sublime Merge, sublime-4)

Three columns, all light:

1. **Locations**: collapsible BRANCHES / REMOTES / TAGS / STASHES / SUBMODULES /
   WORKTREES with counts.
2. **Commits | Files** tabs. Commit list with a colored graph rail on the left,
   subject (bold), author + relative date, badges for tags (yellow), local
   branch (blue), remote branches (grey), changed-file count at the right.
3. **Detail**: summary table (hash, tree, author, date, parents, branches, tags,
   stats `-6 +12`), then per-file collapsible sections with blue hunk headers
   and a **side-by-side diff** (red left, green right, intra-line highlight).
   Diff code uses a smaller monospace than the editor.

## Palette (sampled)

| Role | Hex |
|---|---|
| Editor / terminal / minimap bg | `#303942` |
| Sidebar bg | `#ecedef` |
| Sidebar selected row | `#a0a4a7` |
| Tab bar bg | `#73787e` |
| Active tab | `#dee3e7` |
| Status bar | `#c7cad1` |
| Palette bg | `#e4e5e9` |
| Palette input | `#ffffff` |
| Palette selected row | `#c7cad1` |
| Merge: locations pane | `#dddee0` |
| Merge: commit list | `#ecedef` |
| Merge: selected commit | `#d3e0f0` |
| Merge: hunk header | `#bad4ed` |
| Merge: diff removed / added | `#f0c2c2` / `#c6f0c2` |
| Merge: diff pane bg | `#f3f3f3` |

Syntax colors (to be sampled from a larger crop later): teal keys, light-grey
text, green strings, orange numbers, blue/purple keywords, muted comments.

## Typography

Editor: a macOS system monospace (looks like Menlo / SF Mono) at roughly 12px
logical size with ~1.45 line height. UI: SF (system) at ~12px. To be confirmed
against the user's Sublime settings (`font_face`, `font_size`).

## Principles to hold ourselves to

- Startup < 100ms, one frame of input latency, huge files open instantly.
- Nothing animates unless it conveys state. No toasts, no modals where a
  palette will do.
- Every feature reachable from the command palette; everything has a shortcut
  shown there.
- Terminal and git are panes inside the same window, not separate surfaces.
