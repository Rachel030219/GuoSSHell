# rsHell Design System

## 0. Terminal recovery authority

This document is the current product design authority. It mirrors the approved
`docs/superpowers/specs/2026-08-28-terminal-hidpi-ui-recovery-design.md` and
supersedes the Task21-only authority. The existing Task21 widget selectors stay
valid until their components migrate to the adaptive composition.

For the current Windows UI refinement stage,
`docs/superpowers/specs/2026-09-09-windows-ui-refinement-design.md` supplements this
authority and supersedes only the cross-platform acceptance prerequisite.

rsHell remains a native GTK4/Relm4 terminal-first SSH workspace. This authority
makes no framework or security change: SSH authentication, host-key handling,
credential storage, journaling, import, reconnect, and secret-redaction
semantics remain unchanged.

## 1. Identity and tokens

Terminal content, connection identity, and session state dominate. Depth comes
from opaque Fluent dark tonal layers and boundaries, never floating cards.

| Token | Value |
|---|---|
| font-ui | `"Segoe UI Variable Text", "Segoe UI Variable", "Segoe UI", system-ui, sans-serif` |
| font-terminal | `"Cascadia Mono", "Microsoft YaHei UI", "Segoe UI Emoji", "Consolas", monospace` (GTK CSS stack) |
| font-terminal-default | `"CaskaydiaCove NF Mono"` (single requested monospace family) |
| type-terminal-default | 18 absolute logical px |
| type-root | 15 logical px |
| type-secondary | 14 logical px |
| type-control | 15 logical px |
| type-dialog-title | 18 logical px |
| line-body | 22 logical px minimum |
| line-control | 36 logical px minimum |
| terminal-line-spacing | 2 logical px |
| spacing-unit | 4 logical px |
| border-exception | 2 logical px |
| navigation-compact | 48 logical px |
| navigation-standard | 260 logical px |
| navigation-wide-max | 280 logical px |
| window-startup-target | 80% of GTK-reported monitor geometry |
| window-startup-min | 960 × 640 logical px when the monitor permits |
| window-startup-max | 1920 × 1200 logical px |
| window-startup-margin | 48 logical px total per dimension |
| window-startup-fallback | 1360 × 860 logical px |
| surface-shell | `#202020` |
| surface-command | `#202020` |
| surface-sidebar | `#202020` |
| surface-control | `#2b2b2b` |
| surface-control-hover | `#333333` |
| surface-overlay | `#2b2b2b` |
| surface-scrim | `#121212` |
| surface-tab | `#1e1e1e` |
| surface-terminal | `#1a1a1a` |
| content-primary | `#f5f5f5` |
| content-secondary | `#cccccc` |
| content-tertiary | `#9d9d9d` |
| border-default | `rgba(255, 255, 255, 0.10)` |
| border-strong | `rgba(255, 255, 255, 0.16)` |
| accent | `#60cdff` |
| accent-hover | `#78d6ff` |
| accent-active | `#4ab5e6` |
| accent-content | `#002b40` |
| semantic-error | `#ff99a4` |
| semantic-warning | `#fce100` |
| semantic-success | `#6ccb8e` |
| control-radius | 4px |
| overlay-radius | 8px |
| focus-width | 2px |
| motion-fast | 80ms |
| motion-standard | 100ms |
| motion-focus | 120ms |

Application chrome, commands, tabs, and primary rows use the 15px root size.
Secondary metadata never falls below 14px; entries, menus, and dialog actions
use 15px, while modal titles use 18px with a distinct 15px section hierarchy.
Connection identity uses weight rather than oversized type. Controls provide at
least a 36px readable line box. Spacing and modal composition follow a 4px
rhythm; 2px is reserved for terminal line separation, borders, separators, and
focus treatment rather than general layout spacing.

New terminal profiles request `font-terminal-default` at `type-terminal-default` for
readable terminal text and Nerd/Powerline prompt glyphs on the current Windows
installation. This optional font is not bundled or installed by rsHell. If the
exact monospace family is unavailable, the existing measured `Monospace`
fallback preserves the requested size. Windows CJK and emoji retain their
per-text `Microsoft YaHei UI` and `Segoe UI Emoji` routing; the requested family
is not the `font-terminal` CSS stack, which does not set the drawn canvas font.
Explicit saved profile values and
connection overrides are never replaced by changed built-in defaults. Settings
allows font family and size edits (6–72 logical px in 0.5px steps), with explicit
connection overrides taking precedence over the selected profile.

## 2. Safe interrupt and recovery

Unmodified Ctrl+C is a safety interrupt and sends exactly one ETX byte (`03`),
independent of Kitty or CSI-u negotiation. Ctrl+Shift+C remains copy. Sending an
interrupt never automatically resets a surviving application: a TUI that
catches ETX must remain active, alternate, and usable.

If the next authoritative frame from that surviving session still has display
residue, the pane shows the exact non-secret status `Display mode not restored`
with an accessible Reset display action. Recovery returns to primary screen and
clears enhanced keyboard, mouse reporting, application cursor, hidden cursor,
and stale title while preserving primary text and scrollback.

Session exit, failure, crash, disconnect, and reconnect from the old transport
recover automatically before publishing their terminal state. Terminal,
recovery notice, and disconnected state are mutually exclusive pane layers;
detached terminal controllers receive no further presentation updates.

## 3. Measured terminal geometry and HiDPI

Terminal geometry comes from one absolute logical-pixel Pango font description
resolved on the GTK main thread and carried unchanged into PangoCairo drawing.
Cell width is the ceiling of the larger monospace approximate width, measured
ASCII advance, and scale-aware representative ink width per protocol column.
Cell height is the ceiling of ascent plus descent or representative ink height,
plus `terminal-line-spacing`. No point/device-DPI reinterpretation and no
product 9×18 fallback is authorized.

One measured value drives rendering, cursor, selection, hit-testing, IME,
rows/columns, PTY pixel dimensions, and DPI. Wide cells occupy exactly two grid
cells; fallback glyph advance never changes column identity. Font, effective
scale/DPI, and rendering identity invalidate the metric without recreating the
session or emitting a duplicate unchanged resize.

Product icons remain source-controlled SVG/internal vectors. GTK allocates the
logical icon size while rendering and caching by icon, backend, and effective
physical size; no external payload is introduced.

## 4. Adaptive terminal-first shell

Layout follows the main window's realized logical allocation and reparents
existing controllers without recreating reducer or session state.

A newly constructed main window targets 80% of the first monitor geometry GTK
reports, floored to the 4px spacing grid. The result is capped at 1920×1200 and
normally raised to 960×640, but the 48px total per-dimension monitor margin takes
priority over that minimum on a small screen. GTK's geometry is the complete
monitor rectangle, not a measured work area; the percentage and margin reserve
space for system chrome. If GTK reports no monitor, startup retains the prior
1360×860 default. This selection runs once during root construction and does not
override resizing after the window is realized.

| Mode | Width | Navigation and terminal behavior |
|---|---:|---|
| Compact | `< 900` | 48px navigation rail; connection drawer; icon global actions; pane-action overflow. |
| Standard | `900–1439` | 260px resizable sidebar; compact text/icon actions; terminal owns remaining width. |
| Wide | `>= 1440` | Sidebar may grow to 280px; forms stay capped; terminal receives extra width. |

Crossing a breakpoint preserves the active tab, focused pane, live sessions,
search, selection, and unsaved editor draft. The native title bar alone owns
the product name. The in-content command bar presents New session, Import,
Settings, and concise workspace status without duplicate identity. Session
state belongs to tabs and pane rows rather than a permanent global status strip.

## 5. Navigation, tabs, panes, and overflow

Navigation uses readable text and icons in Standard/Wide and accessible icons
plus a drawer in Compact. Connection rows are tonal list rows, not cards.

Tabs use a horizontal scroller, automatically reveal the active tab, and expose
an accessible overflow list. At least twenty tabs remain keyboard reachable.
Pane actions use priority groups: split, reconnect/retry, and close remain
visible where space allows; diagnostics and edit move to pane-action overflow.
All supported split trees retain non-zero terminal allocations in every mode.

Windows shell actions provide at least 36px outer targets; tab text uses a 36px
content minimum with two 2px state boundaries (40px outer). Tab close/add retain
36px usable width, while embedded 16px icons stay centered rather than stretching
to the button allocation. The Compact rail measures 48px including its boundary
and inset. Command status is a single-line, selectable ellipsis with the complete
non-secret text retained in its native tooltip and selection; it must never grow
the shell or consume the terminal when a multiline operation fails. Full modal
validation remains in the scrolling form body.

| Surface | Required states |
|---|---|
| Command and navigation actions | default, hover, pressed, keyboard focus, disabled, rejected |
| Connection rows | default, hover, selected, focused, empty, error |
| Session tabs and overflow | inactive, hover, active, focus, close, disabled, rejected |
| Pane actions and recovery notice | default, hover, focus, pressed, disabled, pending, success, error |
| Fields | default, focus, populated, disabled, invalid, inherited, explicit override |
| Import and secure interaction | pending, warning, success, rejected, retry, cancel |

Every icon-only action has a non-empty accessible name and tooltip. State text
or native semantics accompanies color and motion.

## 6. Modal behavior

Connection editor, Settings, Import, and secure Interaction share the main GTK
overlay. Each uses an opaque `.modal-scrim` and has a max width of `min(680px, window width - 48px)`.
The header and footer remain fixed while the
body scrolls; grouped fields do not stretch merely because the window is Wide.

Opening a modal makes background widgets insensitive, moves focus to an
intentional first control, contains Tab order, supports Escape cancel, and
returns focus to the trigger. Errors remain adjacent to their field or action.
The controls remain native GTK widgets, not custom-painted form controls.

The connection title inherits the 18px dialog-header hierarchy rather than the
15px connection-row identity style. Multiline Note fields use `surface-control`
on both the TextView surround and text node, with the shared 2px accent focus.
Settings section grids share a horizontal label size group so their input
columns align without fixed label widths. Operational dialog instructions use
`content-secondary` at `type-secondary`, without theme dimming; disabled primary
actions use `surface-control`, `content-tertiary`, and `border-default` rather
than the enabled accent fill. Native sensitivity remains authoritative.

Windows form controls use a single 36px content minimum with zero vertical
padding and 2px boundaries: measured outer height is 40px, the next 4px step
above the former native Entry requisition of 39px. Compound inner buttons do
not repeat the outer minimum. Body gap is 12px, field row gap 8px, column gap
12px. The first section has no top inset; subsequent section separation is
20px (the section selector alone owns the extra inset above the parent's gap).
Body scrollers and ordinary form groups have no additional frame or card fill.
The dialog frame, Note surround/focus, footer separator and danger boundary
remain. Long validation belongs at the end of the scrolling body, never in the
fixed footer. Multiline content is not subject to single-line height limits.

## 7. Motion and interaction

- `motion-fast` 80ms covers hover and immediate acknowledgements.
- `motion-standard` 100ms covers command, navigation, tab, and pane changes.
- `motion-focus` 120ms covers focus and field-boundary changes.
- No other duration is authorized. Motion honors `gtk-enable-animations`, never
  delays dispatch, and never changes sensitivity after input.
- Keyboard activation remains native. Ctrl+Enter saves where the reducer allows;
  Escape closes the active overlay; Enter submits only an enabled action.

## 8. Contrast, depth, and exclusions

Primary operational text must maintain at least 4.5:1 contrast at its rendered
size; focus treatment and non-text state boundaries maintain at least 3:1.
Operational labels use explicit foreground tokens rather than 40–50% opacity.
Native `.dim-label` uses full opacity so ambient theme dimming cannot reduce the
declared `content-tertiary` contrast (including import candidate metadata).
Semantic colors are paired with visible text or an accessible name.

Depth is borders-first and tonal. Gradients, drop shadows, fake Mica/acrylic,
backdrop blur, decorative filters, fake transparency, floating card stacks,
emoji icons, browser infrastructure, and platform-specific title-bar APIs are
not authorized. The terminal canvas remains the opaque `surface-terminal`.

Product SVGs use `currentColor` and contain no external references, scripts,
fonts, animation, raster payloads, gradients, or filters. Missing SVG-loader
support selects the compiled internal-vector backend; malformed assets fail.

## 9. Accessibility and evidence

Native controls retain roles, labels, keyboard navigation, disabled state, and
visible 2px focus. Changed host keys have no acceptance path. Unknown host keys
offer Reject and Accept-and-store only. Secrets never enter widgets,
screenshots, public snapshots, diagnostics, JSON, JUnit, or logs.

Realized visual evidence covers approximately 800×600, 1360×860, and
1920×1080 across Compact, Standard, and Wide states. Screenshot metadata records
the realized logical window and widget sizes, active breakpoint, effective
scale and DPI, measured cell metrics, icon logical size, icon physical source
size, and texture dimensions. It records state identifiers rather than terminal
text, endpoints, paths, credentials, or other secret content.

Evidence must reject terminal ink escaping its assigned one/two-cell rectangle
and any rendered row whose minimum ink separation is below
`terminal-line-spacing`. It must cover empty, connected, twenty-tab, nested-split, modal,
host-key, authentication, failure, and recovery states. Unavailable monitor
scales or hosted platforms are reported as unavailable rather than synthesized.
No accessibility debt or external CI result is claimed without its acceptance
run.
