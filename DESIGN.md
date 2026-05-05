---
name: Colomin

description: >
  A minimal, data-focused CSV editor for macOS. The design language
  prioritises information density and legibility over decoration —
  every pixel earns its place by serving the data.

themes:
  default: colomin-light
  available:
    - colomin-light
    - colomin-dark
    - github-light
    - github-dark

# ─── COLOMIN LIGHT (default) ─────────────────────────────────────────────────
colors:
  background:        "#FAFAFA"   # app canvas behind all surfaces
  surface:           "#FFFFFF"   # table headers, sidebars, panels
  border:            "#F4F4F4"   # all 1 px dividers and widget outlines
  gutter-bg:         "#FFFFFF"   # row-number column background
  status-bar-bg:     "#FAFAFA"   # bottom status bar

  text-primary:      "#1A1A1A"   # body copy, cell values
  text-secondary:    "#8A8A8A"   # metadata, secondary labels
  text-tertiary:     "#B0B0B0"   # row numbers, placeholder text

  accent:            "#3B82F6"   # interactive focus, links, selected column header
  accent-hover:      "#2563EB"   # accent on hover
  accent-subtle:     "#EFF6FF"   # active widget background tint
  on-accent:         "#FFFFFF"   # text / icon on accent-filled surfaces

  state-edited:      "#FFF7ED"   # warm tint on unsaved cell
  state-hover-row:   "#F5F8FF"   # very light blue tint on hovered row
  state-selection:   "#C7DEFF"   # filled selection highlight
  state-danger:      "#EF4444"   # destructive actions, error indicators

# ─── COLOMIN DARK ─────────────────────────────────────────────────────────────
colors-dark:
  background:        "#141414"
  surface:           "#1B1B1B"
  border:            "#383838"
  gutter-bg:         "#1B1B1B"
  status-bar-bg:     "#141414"

  text-primary:      "#E8E8E8"
  text-secondary:    "#A1A1A1"
  text-tertiary:     "#6E6E6E"

  accent:            "#60A5FA"
  accent-hover:      "#3B82F6"
  accent-subtle:     "#172554"
  on-accent:         "#08111F"

  state-edited:      "#3A2410"
  state-hover-row:   "#1E293B"
  state-selection:   "#17335C"
  state-danger:      "#EF4444"

# ─── TYPOGRAPHY ───────────────────────────────────────────────────────────────
typography:
  # Proportional — UI chrome, labels, menus
  body:
    fontFamily: System UI (platform proportional)
    fontSize: 12px
    fontWeight: 400
    lineHeight: 16px

  label-sm:
    fontFamily: System UI (platform proportional)
    fontSize: 11px
    fontWeight: 400
    lineHeight: 14px

  title:
    fontFamily: System UI (platform proportional)
    fontSize: 13px
    fontWeight: 600
    lineHeight: 18px

  heading:
    fontFamily: System UI (platform proportional)
    fontSize: 15px
    fontWeight: 600
    lineHeight: 20px

  # Monospace — data cells, row numbers, cell editor input
  cell:
    fontFamily: System Monospace (platform monospace)
    fontSize: 12px              # default; user-configurable 10–18 px
    fontWeight: 400
    lineHeight: 16px

  row-number:
    fontFamily: System Monospace (platform monospace)
    fontSize: 10px              # cell font size − 2, min 8 px
    fontWeight: 400
    lineHeight: 14px

  cell-editor-input:
    fontFamily: System Monospace (platform monospace)
    fontSize: 10px              # cell font size − 2, min 8 px
    fontWeight: 400
    lineHeight: "cell font size + 4 px"

# ─── SPACING ──────────────────────────────────────────────────────────────────
spacing:
  base: 4px          # smallest unit
  xs:   4px
  sm:   8px
  md:   12px
  lg:   16px
  xl:   24px

  # Component-specific
  cell-h-padding:       6px    # left + right = 12 px total per cell
  panel-h-padding:      8px
  panel-v-padding:      4px
  row-height-default:   30px
  row-height-min:       16px
  column-width-default: 150px
  column-width-min:     40px
  column-width-max:     800px
  gutter-right-pad:     8px
  sidebar-width-default: 300px
  sidebar-width-range:  "200–800 px"
  scrollbar-thumb-min:  24px   # min length in both axes
  scrollbar-thumb-pad-h: 3px   # horizontal clearance inside track
  scrollbar-thumb-pad-v: 2px   # vertical clearance inside track

# ─── SHAPE / RADII ────────────────────────────────────────────────────────────
rounded:
  none:   0px    # cells, row backgrounds, all flat surfaces
  sm:     4px    # buttons, scrollbar thumbs, resize handles
  DEFAULT: 4px

# ─── ELEVATION ────────────────────────────────────────────────────────────────
# No box shadows. Depth is communicated through colour alone.
elevation:
  base:    "{colors.background}"   # app canvas
  raised:  "{colors.surface}"      # panels, headers, sidebars
  overlay: "{colors.surface}"      # dropdowns, menus (no shadow added)

# ─── STROKES ──────────────────────────────────────────────────────────────────
strokes:
  default:
    width: 1px
    color: "{colors.border}"
  focus:
    width: 1px
    color: "{colors.accent}"
  text:
    width: 1px
    color: "{colors.text-primary}"

# ─── MOTION ───────────────────────────────────────────────────────────────────
motion:
  scrollbar-fade-in:
    property: opacity
    rate: "8.0 α/s"         # alpha units added per second on scroll start
  scrollbar-fade-out:
    property: opacity
    rate: "1.5 α/s"         # alpha units subtracted per second after linger
    linger: 600ms           # delay before fade-out begins
  scrollbar-idle-opacity: 0.35   # resting opacity while panel is hovered
  selection-dash:
    property: stroke-dashoffset
    speed: "12 units/s"     # marching ants animation speed
    period: 9               # dash pattern repeats every 9 units

# ─── COMPONENTS ───────────────────────────────────────────────────────────────
components:

  button:
    backgroundColor: "{colors.surface}"
    textColor: "{colors.text-primary}"
    typography: "{typography.body}"
    rounded: "{rounded.sm}"
    borderColor: "{colors.border}"
    borderWidth: 1px
    padding: "4px 8px"

  button-hover:
    backgroundColor: "{colors.state-hover-row}"
    borderColor: "{colors.accent}"

  button-active:
    backgroundColor: "{colors.accent-subtle}"
    borderColor: "{colors.accent}"

  button-danger:
    textColor: "{colors.state-danger}"

  table-cell:
    backgroundColor: "{colors.background}"
    textColor: "{colors.text-primary}"
    typography: "{typography.cell}"
    rounded: "{rounded.none}"
    padding: "0px 6px"
    height: "{spacing.row-height-default}"

  table-cell-hovered:
    backgroundColor: "{colors.state-hover-row}"

  table-cell-selected:
    backgroundColor: "{colors.state-selection}"
    borderColor: "{colors.accent}"
    borderStyle: dashed
    borderWidth: 1px

  table-cell-edited:
    backgroundColor: "{colors.state-edited}"

  table-header:
    backgroundColor: "{colors.surface}"
    textColor: "{colors.text-primary}"
    typography: "{typography.body}"
    borderBottomColor: "{colors.border}"
    borderBottomWidth: 1px

  table-header-active:
    borderBottomColor: "{colors.accent}"

  row-number:
    backgroundColor: "{colors.gutter-bg}"
    textColor: "{colors.text-tertiary}"
    typography: "{typography.row-number}"
    borderRightColor: "{colors.border}"
    borderRightWidth: 1px

  scrollbar-thumb:
    rounded: "{rounded.sm}"
    paddingH: "{spacing.scrollbar-thumb-pad-h}"
    paddingV: "{spacing.scrollbar-thumb-pad-v}"
    color-dragging: "{colors.text-secondary}"
    color-hover:    "blend({colors.text-secondary}, {colors.text-tertiary}, 0.5)"
    color-normal:   "{colors.text-tertiary}"

  sidebar:
    backgroundColor: "{colors.surface}"
    width: "{spacing.sidebar-width-default}"
    padding: "8px"

  status-bar:
    backgroundColor: "{colors.status-bar-bg}"
    textColor: "{colors.text-secondary}"
    typography: "{typography.label-sm}"
    borderTopColor: "{colors.border}"
    borderTopWidth: 1px
    height: 22px
---

## Overview

Colomin is a desktop CSV editor for macOS built with the [egui](https://github.com/emilk/egui) immediate-mode UI framework. The design language is **minimal and data-first**: the spreadsheet grid is the product, and every surrounding element — toolbars, sidebars, the status bar — exists only to serve the data within it.

The aesthetic is deliberately quiet. There are no drop shadows, no gradients, no decorative illustration. What remains is a precise system of flat colour, consistent 1 px strokes, and careful typographic hierarchy.

## Colour Philosophy

The palette is built around two axes: **surface depth** and **semantic state**.

Surface depth uses just two levels: `background` (the app canvas) and `surface` (panels, headers, the gutter). The difference between them is intentionally subtle — a few luminance steps — enough to define structure without drawing the eye away from cell content.

Semantic state colours communicate interactive meaning:

| Token | Light | Purpose |
|---|---|---|
| `accent` | `#3B82F6` | Focus rings, selected column headers, interactive affordances |
| `state-selection` | `#C7DEFF` | Highlighted cell range |
| `state-hover-row` | `#F5F8FF` | Row under the pointer |
| `state-edited` | `#FFF7ED` | Unsaved edit (warm amber wash) |
| `state-danger` | `#EF4444` | Destructive actions |

The `state-edited` warm amber tint is the only warm hue in an otherwise cool-neutral palette. Its job is to make unsaved work impossible to miss — it reads immediately, even in peripheral vision.

Themes ship in four variants — Colomin Light, Colomin Dark, GitHub Light, GitHub Dark — all following the same semantic token structure. The accent shifts from `#3B82F6` (light) to `#60A5FA` (dark) to maintain sufficient contrast on dark surfaces while preserving the same blue identity.

## Typography

Two font families divide the interface by function:

- **Proportional (system UI font)** — menus, labels, sidebar controls, status bar. Text here supports navigation and commands.
- **Monospace (system monospace font)** — data cells, row numbers, the cell editor's input area. Monospace alignment lets column values scan vertically at a glance, the way a spreadsheet user expects.

The default cell size is **12 px**, user-adjustable from 10 to 18 px. Row-number text is always `cell size − 2 px` (min 8 px) to sit visually subordinate to cell content. The cell editor footer line height expands to `cell size + 4 px` to provide comfortable reading in the multiline view.

Weights stay at 400 for data and labels; 600 (semi-bold) for section headings and active column headers only. Restraint in weight keeps the grid readable under high density.

## Shape Language

Colomin uses **almost no rounding**. Table cells and row backgrounds are perfectly rectangular — `border-radius: 0`. The only rounded element is the `4 px` corner on buttons and scrollbar thumbs.

This is deliberate: rounded corners imply containment and card-like groupings. In a dense grid, unnecessary rounding would create visual noise. Flat edges let rows and columns read as the continuous, scannable bands they are.

## Spacing and Density

The grid is designed for **information density**:

- Default row height: **30 px**
- Cell horizontal padding: **6 px** each side (12 px total)
- Border strokes: **1 px** throughout

This is compact but not cramped. The 30 px row height provides enough vertical whitespace for legibility at 12 px font size while keeping more rows visible without scrolling. The 6 px cell padding gives cell text room to breathe without wasting horizontal space.

The layout follows a **4 px base unit**. All spacing values are multiples of 4.

## Elevation and Depth

There are no shadows anywhere in the interface. Depth is expressed through **colour only**: the background is slightly darker than the surface layer above it. This keeps the aesthetic flat and direct — consistent with a tool where the data is the focus.

The row-number gutter sits visually behind the table body, reinforced by its tertiary text colour. The sidebar uses the surface colour to read as a distinct panel without a drop shadow separating it.

## Motion

Motion in Colomin is functional, not decorative. Two animated systems exist:

**Overlay scrollbars** behave like macOS native scrollbars: invisible at rest, fading in at `8 α/s` when the user scrolls, then lingering for 600 ms before fading out at `1.5 α/s`. While the cursor is inside the table area, the thumbs remain faintly visible at 35 % opacity. This keeps the scroll affordance discoverable without cluttering the view.

**Marching ants selection** — the dashed border around the selected cell range animates at 12 units/s with a 9-unit dash period, matching spreadsheet conventions (Excel, Numbers) and communicating "active selection" unambiguously.

No other motion is used. Hover state changes are instantaneous.

## Components

### Table Grid

The table is the primary surface. Headers use the `surface` background with a 1 px `border` bottom rule. When a column is sorted or focused, that rule recolours to `accent`. Rows alternate between `background` (default) and `state-hover-row` (pointer contact). The selected range fills with `state-selection` and gains a 1 px dashed `accent` border (animated).

Column resize handles occupy the bottom 4 px of each header cell. They are `border`-coloured at rest and `accent`-coloured on hover — a thin but discoverable affordance.

### Gutter (Row Numbers)

The gutter is a fixed-width column on the left, permanently visible. It uses `text-tertiary` for the row numbers and a 1 px right `border` to separate it from cell content. On row-resize hover, its bottom border recolours to `accent`, consistent with the column resize treatment.

### Sidebar (Cell Editor)

The cell editor opens as a right-hand sidebar. Its default width is 300 px, freely draggable between 200 and 800 px. The background is `surface` — the same level as the table header — which visually places it "above" the app canvas. The input area uses monospace type matching the table cells for visual consistency.

### Buttons

Buttons use a minimal outlined style: `surface` fill, 1 px `border`, `4 px` radius. Hover promotes the border to `accent` and shifts fill to `state-hover-row`. Active state uses `accent-subtle` fill. Destructive buttons recolour their label to `state-danger`; the border and shape remain identical to avoid jarring contrast.

### Scrollbars

Custom overlay scrollbars match the macOS idiom: translucent thumbs, no visible track, no scrollbar arrows. Thumb colour progresses from `text-tertiary` (normal) through a blend of `text-secondary/text-tertiary` (hover) to `text-secondary` (dragging). The `4 px` thumb radius softens the only moving element on screen.

### Status Bar

A 22 px bar at the bottom of the window. `label-sm` typography, `text-secondary` colour, separated from the table by a 1 px `border` top rule. Used for row/column counts and cursor position — reference information, never controls.

## Design Principles

1. **Data first.** The grid occupies maximum available space. Chrome elements are narrow and static.
2. **Flat, not lifeless.** No shadows or gradients, but colour and stroke provide clear hierarchy.
3. **Semantic colour.** Every colour token has one job. No colour is used decoratively.
4. **Monospace for data.** All user-entered values render in monospace. Proportional type is reserved for UI.
5. **Motion as signal.** The two animated systems (scrollbars, marching ants) communicate affordance and state — not brand personality.
6. **Theme-agnostic tokens.** The semantic token layer (accent, surface, state-*) maps identically across all four themes; only the resolved hex values differ.
