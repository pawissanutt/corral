# Visual artifact gallery

Press `V` on the board, or `Ctrl+G` in the artifact finder (`corral artifacts`), to open
the gallery. It shows pinned visual artifacts first, followed by the 40 newest unpinned
visuals. Raster images display inline; HTML reports display a screenshot captured when
the producing run calls `corral done`. Enter always opens the original artifact through
the configured viewer.

The board shortcut can be remapped in Settings or with `ui.keys.gallery`; its default
is `V`. Gallery filtering keeps literal text input regardless of board key remaps.

Use arrows to select a tile, Page Up/Down to change pages, and type to filter by task,
project or title. Tab switches between recent plus pinned visuals and pinned visuals
only. `*` pins or unpins the selected output, `Ctrl+A` opens the full artifact finder,
and Escape returns to the previous screen. Resolved follow-latest pins mark their
current output, not the historical version on which the pin was created.

HTML capture uses `google-chrome`, `chromium` or `chromium-browser` on PATH.
`CORRAL_CHROME` selects a different Chrome-compatible executable. Each capture uses
a disposable profile and the immutable HTML snapshot, a 1200x800 viewport, a 10-second
browser capture deadline and a 20-second process deadline. No installed browser,
an unsuccessful capture, or an invalid PNG produces a diagnostic and a visible
unavailable-preview tile; the original report remains openable and task completion
is not blocked. Reports should be self-contained: relative worktree files are not
copied into the artifact snapshot.

Screenshots are derived caches next to the artifact snapshot, named
`<snapshot>.thumbnail.png`. They never replace the HTML or move a following pin.
Deleting a cache is safe; a repeated completion capture can regenerate it. Historical
HTML artifacts without a done-time screenshot remain openable from the gallery.

For a report regenerated across runs, pin its artifact with:

```sh
corral pin T-042:R1:dashboard-report.html --follow-latest --label "Build health"
```

Keep the report title stable across runs so that the pin follows the latest snapshot
even when each run has a different worktree path.
