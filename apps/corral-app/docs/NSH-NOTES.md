# NSH 0.5.0 notes

What building the shell and content taught us about NSH that the docs do not say plainly. Each entry names where it
was checked. "Observed" means seen in a build or a run on this host; "read" means found in the sources
and not yet exercised.

## Toolchain and build

| Topic | Finding | Checked |
|---|---|---|
| rustc | NSH 0.5.0 crates refuse rustc below 1.97 (`nsh-core@0.5.0 requires rustc 1.97`). Corral pins 1.94, so this app has its own `rust-toolchain.toml`. | observed, build output |
| Linux window | The template makes the library crate depend on `nsh-platform-linux`, which needs the GTK 4 development packages even for web, terminal and headless builds. The platform crates are behind the `desktop` feature here. Whether `nsh run --target linux|windows|macos` still works with the gated dependency is unknown. | observed (failure without the gate); the rest not checked |
| Web | Needs `wasm32-unknown-unknown`, `clang` and `llvm-ar` (zstd is C). `nsh` exports the bare names `clang` and `llvm-ar` for that target, so a `.cargo/config.toml` `[env]` entry needs `force = true` to point elsewhere. | observed |
| Tests | `nsh test` runs `cargo test` and then `*.test.tsx`. `--target tui` runs on an in-memory terminal and `--target web` in headless Chrome; both always run as the desktop idiom, so the phone idiom on the web cannot be reached that way. | observed |
| Test file location | `nsh test <file>` needs the file inside an app directory ("no app around it (an App.tsx or nsh.json)" otherwise). `app.screenshot(path)` writes a PNG on `--target web` and a Cell grid as JSON on `--target tui`, which makes both targets inspectable without a display. | observed |
| Web idiom | The page decides the idiom once: `matchMedia("(pointer: fine) and (hover: hover)")` is desktop; otherwise the shorter side of `screen` under 600 px is phone, else pad. Viewport width plays no part. A headless Chromium without a fine pointer therefore gets the phone shell at any window size. | observed; rule read in the packaged `nsh.<digest>.js` (`measureIdiom`) |
| Fonts | Each shipped font file registers as one face, at the single weight in its OS/2 table, under the family name in the file. A variable font is not treated as a range of weights: the app ships one static file per weight. A subset variable file whose name table says "… ExtraLight" registers under that name. | observed in the browser (`document.fonts`), `docs/fonts` |
| Font styles | An italic file registers as the italic face of its family (the web passes ttf-parser's `is_italic` to `FontFace`), so Cascadia Mono's regular, bold, italic and bold italic files are one family that `font_weight` and `italic()` choose from. `assets!()` registers only `ttf`, `otf`, `woff` and `woff2` as fonts; any other file in `assets/` (the font's licence) is a plain file asset. | read, `nsh-macros/src/assets.rs` (`FONT_EXTENSIONS`), `nsh-platform-web/src/text.rs` `register_font` |
| Font fallback | A `font_family` value is a CSS list; text is set in the first family a platform has, and a character that family lacks in the next family that has it, so a fallback glyph comes with that font's own advance. A monospace grid has to box such characters to their cells. | read, `nsh-core/src/style.rs` `font_families` |
| Inline JS modules | `wasm_bindgen(inline_js = …)` ships an addressable ES module in development and hashed web packages. A same-origin module can register its own `import.meta.url` as a module service worker; use its directory scope, not a root intercept. Cross-origin asset/CDN hosting cannot register it. | read, `src/client/web/push.rs`; real vendor push delivery unverified |

## Names that collide with the prelude

`nsh_core::prelude::*` exports a function `color(..)` and a type `Overlay`. A glob import of an app module
that has the same names fails with E0659. Import app items by name (`use crate::ui::tokens::{color, Token};`,
`use crate::model::Overlay;`). Source: `nsh-core/src/element/ambient.rs:291`, observed by the theme unit.

## Keys

| Finding | Checked |
|---|---|
| `on_key` panics on a malformed or reserved keystroke instead of returning an error. Every keystroke string is a literal here. | read, `element/ambient.rs:1528` |
| With the default `When::Auto`, a binding does not fire while a text field has the focus if the field would use the key. `Keystroke::edits_text` counts every non-character key as editing, `escape` included, so a global Esc needs `When::Always`. Letter keys use `When::NotEditing`. | read, `keymap.rs`; behaviour covered by `tests/shell.rs::typing_in_ask_does_not_navigate` |
| When several bindings match, the newest wins (`element/keys.rs:30`). A `Dialog` or `Command` opened later takes Esc before the app's global binding. | read |
| `on_key` fires on desktop platforms only; on phones the bindings are kept but no key reaches them. | read, `element/ambient.rs:1520` |
| A shifted character is named by its unshifted key: `on_key("?")` panics, so the app binds `shift+/`. The web key mapper uses `code: "Slash"` and the shift modifier for a real `key: "?"` event. | read, `nsh-core/src/keymap.rs`, `nsh-platform-web/web/nsh.js` `keyCode`, `src/state.rs` |
| For headless browser key tests, send the real key and code, such as `key: "?"`, `code: "Slash"`, `shiftKey: true`. The web mapper uses the code for shifted keys, so a claim that `key: "/"` alone prevents a match is not established when the same code is supplied. | read, `nsh-platform-web/web/nsh.js` `keyCode` |
| **List-key scope:** the app binds list moves, Enter and answer digits only with list focus or no focused control, and no overlay/confirmation. Tab and Space remain platform controls. Global destination/domain letters use `When::NotEditing`; `.` is a row action menu in live Now/Review list scope, Changes elsewhere. | read, `src/state/keys.rs`, `src/model/commands/keys.rs` |

## Components

| Finding | Checked |
|---|---|
| Builders generated by `#[component]` (Button, Toggle, Tab, Command) have no `test_id`. The id goes on a view inside the control; the test driver's `press` walks up to the nearest pressable. | read, `nsh-macros`; observed through the passing tests |
| `nsh-testing` has no way to press the system `TabBar`. Phone tests run as a phone browser (`Os::Web`, `Idiom::Phone`), where the shell draws its own tabs. The iOS and Android tab bar path has no automated check. | observed (test failure before the change) |
| `TabBar` takes no room outside iOS and Android, phone browsers included, so the phone shell holds both the system bar and app-drawn `Tabs`, chosen by `os()`. Badges on the system bar are not documented. | read, `modules/nsh-mobile` |
| `DockAreaLayout` has no public JSON form. `to_state`/`from_state` give `DockAreaState`, which needs `serde_json` to become text. The stored layout is the dock's JavaScript JSON shape built from `nsh_core::JsonValue` and read back with `nsh_core::element::from_json`. | read, `nsh-base/src/dock/` |
| `DockArea` switches to its one-group presentation below 600 pt of its own width. An app that chooses the arrangement itself passes `compact(Some(false))`. | read, `elements/dock` |
| No icon set ships with NSH. The app mark, chevrons and grips are drawn from views and borders. | read |
| Text size and colour do not pass from a view to the texts inside it. Set each text's style explicitly; `kit::text::rich_small` and `rich_at` set their paragraph style. | read, `src/ui/kit/text.rs`; view inheritance not found in the docs |
| `flex_shrink` defaults to 0, unlike CSS. A plain view in a row that must give way needs `flex_shrink(1.0)` and `min_w(0)` or `flex_1`. [INFERENCE] A fixed-width part can blank a narrow pane. | read, `element/style.rs`, `src/ui/kit/page.rs` |
| Input placeholder, caret and selection colours take one colour, not a light and dark pair. Shadow layers take a plain colour too. | read |
| An `effect` runs once while the component is being built, before its elements are mounted, and an `ElementRef` command sent then reaches nothing. To focus a field that was just made, defer with `after_answer`, which also runs after a closing `Dialog` has handed the focus back. `auto_focus` fires once per element and on the web waits for a timer. | read, `element_ref.rs`, `nsh-base/src/popup.rs`; observed (the field was not focused before the change) |
| `flex_1()` is `flex: 1 1 0`: it ignores the element's own size. A box that should start from its content's width and then give way uses `flex_auto()`. | read, `element/style.rs`; observed (clipped top bar at 900 pt) |
| A nested text's background (`.bg` on a run) paints from the font's ascent to its descent around the baseline, as CSS paints inline boxes, not over the line height. With a leading taller than the font's content box, rows of filled runs show stripes of the page between them. Cascadia Mono's content box (typo metrics, 1.16 em) is shorter than its cell (1.32 em, which its box drawing and blocks fill), so the styled screen fills each band of background with a view one row tall instead. | read, `nsh-core/src/text.rs` `TextBackground`; font tables read with Python |
| A one-line text clamp wraps at word boundaries before cutting, so it cannot clip a code line mid-word at the cell edge. Review lays the line out wider than the cell with `EllipsizeMode::Clip` inside an `overflow_hidden` view. | read, `src/screens/review/diff.rs` `clip` |
| A phone sheet's content can be built before it opens. The pairing sheet builds content for its own sheet kind rather than relying on which sheet is open at construction time. | read, `src/screens/settings/devices.rs` `sheet_host`, `sheet_content` |
| [INFERENCE] NSH has no blur or animated gradient. The decide bar uses the plain page colour and the loading skeleton pulses opacity instead. | read, `src/ui/kit/page.rs` `bar`, `src/ui/kit/notice.rs` `skeleton`; absence of APIs not established |
| Canvas drawings can take stroke and fill colours from the canvas view. Usage sets adaptive colours on its canvas layers so the chart follows light and dark appearance. | read, `src/screens/usage/chart.rs` `layers` |
| **Web input names:** NSH 0.5.0 puts `aria-label` on the wrapper, not the inner input. The placeholder names the web input; settings fields therefore repeat their label as placeholder and put hints beneath. Pairing codes are not numeric, so a number-pad keyboard is unsuitable. | source finding DW-16 in SettingsFirstRun handoff; `src/screens/settings/parts.rs`, `enrolment.rs` |
| **Web disclosure state:** the web renderer drops `aria-expanded` when `expanded` is `false` (`updateAria` in `nsh.js` removes any attribute whose value is `false`), so a folded disclosure such as `btn-closed` or `btn-waits` exposes no collapsed state; only the open state reaches screen readers. Still present in 0.5.1. | read, `nsh-platform-web-0.5.0/web/nsh.js` and 0.5.1 `updateAria` |

## Storage

| Finding | Checked |
|---|---|
| Web Storage uses `localStorage`, with keys prefixed `nsh:<app id>:`. This app's destination key is `nsh:com.example.corral:corral.destination`. | read, `nsh-platform-web/src/storage.rs`, `nsh-engine/src/local_storage.rs`, `nsh.json`, `src/state.rs` |
| **Durable client state:** NSH Storage is not the outbox/cache transaction boundary. Browser `corral-local-state` v1 uses `records`/`meta` stores and acknowledges transaction completion; native `durable.json` uses a retained lock, file/directory sync and atomic replacement. Pairing is separately encrypted in `corral-app` v1, `credentials`. | read, `src/client/{durable,durable_web,durable_native}.rs`, `web/keystore.rs` |

## Web address

| Finding | Checked |
|---|---|
| `set_app_url_with(url, HistoryBehavior::Push)` makes a history entry of its own; plain `set_app_url` rewrites the current one. A push in the turn that closes a dialog (the palette's command) still lands as one entry after the dialog's is gone back over. | observed in headless Chromium (`history.length`, Back and Forward), `nsh-platform-web/web/nsh.js` `syncHistory` |
| Back or Forward between this load's entries arrives as `next_app_url_change()`. After a reload the entries of the earlier load are kept by the browser, and going back to one arrives as `next_url_scheme_open()` instead (a link handed to the app), so an app that follows its address awaits both. Each takes one waiter: one task per call. | read, `nsh.js` `popState`, `nsh-core/src/url_scheme.rs`; observed (Back after a reload) |
| `app_url()` is an answer to await, so the place a page was opened at is read from `location.pathname` through `web-sys` before the first screen is built. | read, `nsh-core/src/element/ambient.rs`, `src/state/address.rs` |
| A page opened at a path of several segments loads its files only when `nsh.json` names the site's path (`web.base`); the dev server always serves them from `/`. | read, nsh.dev `docs/web` "Links into the app"; observed on the dev server |

## Terminal

| Finding | Checked |
|---|---|
| A box with content takes its content's height; an explicit height is kept only by empty boxes, scroll views and clips. | read, `core/terminal_layout.rs` |
| Borders are drawn only on "cards" (padding of 12 pt or more, or two or more rows of content). A border elsewhere may be dropped. | read, `docs/tui` |
| `cell_size()` gives the Cell size in points and is not in the prelude (`nsh_core::element::cell_size`). | read, `element/ambient.rs:422` |
| A `size_full()` view keeps its height only while it is empty. A view that measures itself with `on_layout` and then renders by that measure must also grow into its parent (`flex_grow`), or its second measure is of its own content. | read, `core/terminal_layout.rs`; observed (the Now dock fell back to two panes at 120×36) |
| A box with its own background fill never gets a border. A pane edge is an outer box with only the border and an inner box with the fill. | read, `core/terminal_layout.rs` (`own_insets`) |
| The terminal never shows the app's colours as values. A saturated colour becomes the terminal's accent of that hue; a neutral colour is the foreground, **muted when its lightness is within 75 L\* of what it is drawn on**. `Ink2` on a dark page is therefore dim. Text that must read as text needs `Ink`. | observed (ANSI capture: `SGR 2` on every `Ink2` run); `docs/dev/tui.md` "Colors" |
| That holds for 24-bit text too: only canvas pictures keep their colours (at the terminal's depth). A pane's styled screen in the terminal form therefore shows each run as an accent, the foreground or dim, keeps bold, italic, underline and strike-through, and a run's own `.bg` becomes a band (a step or a tint of the ramp) or, without bands, reverse video in the background's accent. There is no way to ask for reverse video directly. | read, `nsh-platform-tui/src/palette.rs` (module doc, `ink`), `render.rs` (glyph background) |
| Fills are bands (steps of the terminal's own background) only when the terminal answers the `OSC 11` background query and has 256 colours or more. Otherwise the look is lines, bold, dim and reverse video. `NO_COLOR` set to anything removes all colour. Both looks have to be checked. | observed in tmux with and without a reported background; `docs/tui` "Colors and themes" |
| `nsh run` replaces the development session already on its port (8790 by default): a second `nsh run` in the same app ends the first. Checks beside a running session need `--port`. | observed (`replacing the NSH development session on port 8790`) |
| `nsh test --target tui` screenshots are drawn with no known background (the look without bands: fills are the standard dark grey), so they do not show what a terminal that reports its background shows. A real run inside a private tmux with `window-style` set does. | observed (Cell JSON: `bg` is `reset` or `darkgray` only) |
| A text of `─` on one row, or of `│` on every row inside a filled box, is a divider: drawn in the frame's line colour and joined to the lines its ends touch (`├ ┤ ┬ ┴ ┼`). The terminal shell builds its ruled grid from these (`ui/term.rs` `hrule`, `vrule`), and the dock's resize handles join them too. | read, `render.rs` `rule_axis`, `holds_rule`, `join_lines`; observed |
| Inside a bordered box, gaps and vertical margins take no rows, and a box around a single text keeps none of its vertical padding. Horizontal padding and margins are one Cell from 8 pt and two from 24 pt, never more. Exact space is a box of that size. | read, `core/terminal_layout.rs` `space_between`, `own_insets` (`compact`); observed |
| Only overlays get a title in their frame. A title in a page frame's line is an absolute sibling over the line, whose opaque page-coloured fill clears the line under it. | read, `render.rs` (title from `accessibility.label` of an overlay's box); observed |
| A modal's frame leaves cut words of the page beside it: before a modal is drawn the page's text Cells are forgotten (`text_cells.fill(false)`), so the "left out whole" rule of the docs applies to anchored popups only. The palette therefore takes the content frame's own lines. | read, `render.rs:171`; observed (`Not bu` beside the palette) |
| An opaque fill clears the symbols under it but keeps their background colours, so a box over a page shows the page's bands through. | read, `render.rs` (`clear_symbols`); observed (tab bands under the narrow Ask) |
| A text field's placeholder is drawn line by line as given and never wraps. | read, `text_input.rs:561` |
| With no bands, an app fill on a selected row becomes the standard grey and the reversed selection draws its text in that grey. Rows and tabs leave their selected look to NSH. | observed (16 colours, ANSI capture `SGR 7` with `48;5;8`) |
| `tmux capture-pane` drops trailing blank Cells and their background, so a band that ends in blanks shows short in the screenshots (the disconnected top bar) though the terminal draws it across. | observed (`.ansi` line ends with the background set and no Cells after it) |

## Not available on this host

Native window APIs, tray, global shortcuts and second windows (`nsh_core::element::desktop`)
need a desktop backend build, requiring GTK 4 here or a supported Windows/macOS machine.
The app implements a root-owned global Ask shortcut and tray. Setting
`WindowAttributes.close_on_press = Some(false)` plus `on_close_press(...hide())` keeps the root/shortcut alive after window
close; tray Quit ends the app. Source: `src/os_integration.rs` and `os_integration/tray.rs`.
This is source-level integration, not native runtime verification on this host. TUI/web
do not install these hooks.

Inbound share targets are not supplied by NSH 0.5.0: Android routes `ACTION_VIEW`, iOS
URL contexts do not carry share-extension payloads, and web packaging has no
`share_target`/manifest override. See the [app guide's source evidence](../README.md#native-entry-points).
