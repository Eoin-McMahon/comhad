# Usage

Everything comhad does, and the keys that do it. See [configuration.md](configuration.md) for
bookmarks, theming and remapping keys.

## The layout

The S3 pane is the main view. A **preview** pane sits alongside it (`p` opens it, or hides it again
if it's already on the Preview tab). A **jobs** pane along the bottom shows live progress for
every download/upload/zip job for the session; `tab`, or `4`, into it to browse past jobs, cancel or
retry one, and open or reveal a finished one in Finder rather than hunting for it in the local pane.

A **local filesystem** pane is available for browsing to a file to upload without drag-and-drop or
typing a path, but it's off by default. Press `L` to bring it in (splitting into a three-column
local/S3/preview layout) and `tab` to switch focus between it and the S3 pane.

Dropping a file from Finder onto the terminal window works regardless of whether `L` is on: comhad
catches it via bracketed paste and offers to upload it into the S3 pane's current prefix.

**Mouse** works alongside the keyboard: click a row to select it (a second click opens/enters it),
`shift`-click to mark everything back to the anchor, `ctrl`/`cmd`-click to pick a single row out,
scroll to move the cursor, and click a segment of the S3 pane's `s3://bucket/path/…` breadcrumb
title to jump straight there. Set `[ui] mouse = false` in `config.toml` to turn it off, see
[configuration.md](configuration.md).

Pane 3 is two tabs, both labelled at the top: **Preview** (`p`) and **Info** (`i`). Info shows name,
key/path, size, last-modified, and whatever metadata the backend returns: S3 gives ETag,
Content-Type and Storage Class, but the field list is generic, so a future non-S3 backend can return
entirely different metadata without any UI changes. Switching tabs doesn't depend on what's
previewable: if a file's too large or binary, `p` still says so and `i` still gets you its info.

For a directory or prefix, Info also breaks down **what it's made of**: a stacked bar by file type
with a legend giving each type's share, bytes and file count, the five largest named and the rest
folded into "other". A prefix that's 900 GB of parquet next to 40 MB of JSON manifests is a
different thing to browse than the same total spread over a million logs, and the total alone can't
tell you which you have. It costs nothing extra: the breakdown is computed from the same listing
the totals already needed.

Preview skips anything over 5 MB (showing "file too large to preview") so a huge object never adds
lag. Text and config files are read as a bounded 4 KB snippet rather than the whole file, and
recognised source types are syntax-highlighted off the render loop. Images (`png`/`jpg`/`gif`/`bmp`/
`webp`, same 5 MB cap) render inline via `ratatui-image`, using whatever graphics protocol your
terminal supports (Kitty, iTerm2, Sixel) and falling back to halfblock ASCII otherwise.

When a file can't be previewed (binary, or past that size cap) the pane says so and then shows the
file's **byte structure** instead: a chart of Shannon entropy, one bar per sampled window, left to
right through the file. Low bars are runs of one byte (zero padding, sparse regions), middling bars
are text and structured records, full bars are compressed or encrypted data. So a zip reads as a
small header followed by a flat wall, a disk image as payload with padding at the end, and a 2 GB
object you could never open still tells you what it's made of. A one-line verdict under the chart
says the same thing in words. Small files are mapped end to end; anything larger is sampled with 48
ranged reads of 4 KB each (~192 KB, whatever the object's size), and the caption says which you're
looking at. Only the byte histogram is used, never the content.

Once a listing runs more than about three screens long, the pane's scrollbar becomes a **minimap**
of the whole thing: marked rows, staged copies and cuts, and anything currently transferring appear
as coloured pips at their position in the full listing, with the visible window lit behind them. In
a prefix with thousands of objects it answers the question a scrollbar can't: not where you are,
but where the things you selected ended up. Shorter listings keep the plain scrollbar.

## Selecting files

Three ways, and they share one anchor, so mixing them behaves the way you'd expect:

* **One at a time.** `space` toggles the hovered row; `ctrl`/`cmd`-click toggles a clicked one.
  Neither disturbs anything already marked.
* **A range.** Mark one row with `space`, move wherever you like (no modifier held, `j`/`k`,
  `ctrl+d`, `G`, all of it), then press `V` to mark everything in between. `shift`-click does the
  same thing with the mouse. The anchor is wherever you last picked a single row, and it stays put
  once a range closes, so a second `V` further down grows the same run. Ranges are additive, so
  rows picked out earlier with `space` survive.
* **Visual mode.** `v` anchors where you are and re-marks the whole range on every cursor move,
  vim-style, so you sweep with `j`/`k` and the selection follows. `v` again or `esc` exits. This one
  *replaces* the selection as it sweeps rather than adding to it, which is what vim does.

Selection lives in one pane at a time: marking in the local pane drops whatever was marked in the S3
pane, so a forgotten selection in the pane you're not looking at never rides along into the next
bulk download. `esc` clears the lot.

## Move, copy, and delete

Mark items (`space`) or just hover one, then:

* `y` copies them to a clipboard, `x` cuts them; either way, navigate anywhere (same pane, a
  different directory, the other pane entirely) and press `P` to paste. Staged items render in a
  distinct colour (green `⧉` for copy, red `✂` for cut) so it's obvious what's queued. Pasting works
  within local, within S3, and in either direction between them; a cross-backend move transfers the
  file first and only removes the source once that transfer actually succeeds. While anything's
  staged, every pane you navigate to grows a greyed, italic `+⧉`/`+✂` ghost row per staged item,
  previewing where it'll land before you ever press `P`.
* `D` permanently deletes the marked/hovered item(s), no undo.
* `Y` copies the hovered item's `s3://bucket/key` (or local absolute path) to your OS clipboard.

Every one of these confirms first. In the confirm popup, `tab`, `←`/`→` or `h`/`l` move between
Yes and No. For a download, or an upload of a single item, `r` changes the name it is saved as: the
file name, the zip name, or the uploaded file or directory name. Text prompts edit like a normal text
box: arrows, `home`/`end`, `ctrl+a`/`ctrl+e`, `alt+←`/`alt+→` (or `alt+b`/`alt+f`) by word,
`ctrl+w`/`alt+backspace` to delete a word, `ctrl+u`/`ctrl+k` to cut to the start/end. Pasting runs as a background transfer job, same as
upload/download: it shows up in the jobs pane with a spinner while running, `→` once a
same-store copy/move lands, and `c` there cancels it.

## Open in `$EDITOR`

Press `O` on a hovered file (local or remote pane) to edit it in `$VISUAL`/`$EDITOR` (falling back
to `vi` if neither is set). comhad suspends the TUI, same as dropping to a shell, runs the editor
in the full terminal, and redraws once it exits.

* **Local files** edit in place, no round trip.
* **Remote files** download to a temp file first; on exit, if the file's mtime changed, comhad
  uploads it straight back to the same key (no confirmation prompt, saving in your editor, e.g.
  vim's `:w`/`:wq`, already *is* the explicit "yes, write this" action) and deletes the temp file
  either way.

This follows the same convention as `git commit`/`crontab -e`: comhad just spawns `$EDITOR <path>`
and waits for it to exit, no per-editor special-casing. That works immediately for a terminal
editor (vim, nano, `emacs -nw`), since those block naturally until you quit. A **GUI editor** (VS
Code, Sublime Text, Zed, ...) normally forks and returns right away, so comhad would never see a
change; give it its own wait flag so the child process actually blocks:

```bash
export EDITOR="code --wait"   # VS Code
export EDITOR="subl --wait"   # Sublime Text
export EDITOR="zed --wait"    # Zed
```

Without `--wait` (or your editor's equivalent), `O` still works, it just returns immediately with
nothing changed yet, so nothing uploads. Nothing is ever silently overwritten either way.

## Sync

Press `s` to open the sync dialog. A banner up top shows the direction at a glance: a lit pill marks
the source, a plain one the destination, with a big arrow between them, and a colour-coded count row
underneath doubles as the legend: **`● N` green** add (missing on the destination), **amber** update
(present but a different size, or the source is newer), **grey** unchanged, and **red** skip (exists
only on the destination, shown for awareness but never deleted). Below that, every file gets the same
git-diff status icon in its row, in both scrollable panels, bordered bright for the source side and
dim for the destination, the same colour pairing as the banner pills. An add also projects onto the
destination panel in green, so you can see the file appear on the side it's about to land on.

`tab`/`d` flips the direction (local→remote upload ⇄ remote→local download) and rescans, `enter` runs
it as normal transfer jobs, and `esc` closes.

Give a bookmark a `local_path` and the local pane opens on the directory that bucket pairs with, so
`s` diffs the right two trees straight after connecting.

## Deep filter

`/` filters the current directory by name, fuzzy (characters just need to appear in order, not
contiguously, so `hlo` matches `hello.csv`) and instant, since it's only filtering what's already
loaded, with matched characters highlighted.

The first non-empty filter also kicks off a one-time recursive scan under the current prefix, cached
for the rest of that filter session, so every keystroke after that is still an in-memory re-filter.
Any match found elsewhere is appended below the normal listing with its relative path in a distinct
colour, rather than only ever surfacing whichever copy happens to be a direct child, so filtering
for `hello.csv` with one copy at the root and another under `archive/2024/` shows both. `enter` on
one of those rows jumps to its real location, and everything else (marking, download, rename,
`y`/`x`/`P`, `D`) works on them directly, since they carry their real key/path.

## Confirmations and events

Every write action asks "are you sure?" with the destination/source path on its own highlighted line
and **Yes**/**No** buttons; `tab`/arrows flip which is selected, `enter` activates it, or press
`y`/`n`/`esc` directly. Delete starts on **No**, since it's the one action with no undo; everything
else starts on **Yes**.

The footer shows the most recent status message (green for success, red for failure) for a few
seconds. Press `E` for the full events log for the session: every status message, newest first, with
the complete error chain and connection diagnostics kept under any failure rather than just the
one-line summary.

Otherwise the footer is the short keybind bar, and it echoes what you press: an action that's on the
bar (`?`, `/`, `L`, `p`, `i`, `q`) lights up green in place, and any other action (download, paste,
delete, sync) flashes as a green chip on the right edge with its key and name before fading. It
makes a session followable when someone else is watching or you're recording a demo. Only actions
flash; getting around (`j`/`k`, `h`/`l`, `gg`, `ctrl-d`, switching pane focus) doesn't, since the
thing that moved already shows it. The bar's keys come from your config, so remapped binds show the
key that actually works.

## The jobs pane

Every download, upload, zip, paste, background rename and directory delete becomes a row in pane
`4`, newest first, with a live progress bar and a throughput sparkline across the top while anything
is moving. Focus it (`4`, or `tab` round to it) and the row under the cursor gets its own keys:

* `c` cancels it. A download or zip stops mid-stream and deletes the partial file it was writing; an
  upload, copy, move or delete stops at the next file boundary, so it never leaves a half-written
  one behind. Objects an upload had already finished stay put.
* `r` runs it again, reusing the row rather than adding a second one. Downloads, uploads and zips
  can be retried. Moves and deletes can't: a partly-finished one has already consumed some of its
  own source, so re-running it from the top would not be the same operation.
* `d` (or `del`) dismisses a finished row, `C` clears every finished row at once. A running job has
  to be cancelled before it can be dismissed, so nothing keeps transferring off-screen.
* `L` points the local pane at the directory the job is writing into and puts the cursor on the
  file, so you can watch it land without navigating there yourself.
* `enter`/`l` opens the job's local file or folder. `f` reveals it in Finder, and works mid-transfer:
  a part-written file reveals as itself, and before the first byte lands it falls back to the
  directory it's going into.

These shadow their global meanings (`c` switch bookmark, `r` rename, `d` download, `L` toggle the
local pane) for as long as pane `4` holds focus, and only while it has rows. Everywhere else they
mean what they always did.

### Where a transfer shows up

A job also renders on the file itself, in whichever pane it's in, and the two sides show different
things so the direction is never ambiguous:

* **The destination fills.** A download fills its row in the local pane, because that's where the
  bytes are landing. An upload fills its row in the S3 pane. The bar is the file's own row, drawn in
  place rather than as a separate widget.
* **The source spins.** The object a download is reading, or the local file an upload is sending,
  swaps its file-type icon for the jobs pane's spinner. It's busy, but it isn't growing, so it
  doesn't get a bar.

A download's destination doesn't exist in the local listing until the job finishes, so until then it
appears as an **incoming row** below the listing: the file name with the same filling bar, so the
pane you're watching it arrive in is never the one place it isn't shown. When the job completes the
listing refreshes and the row becomes the real file. Press `L` in the jobs pane to point the local
pane at that directory in the first place.

## Keybindings

Defaults; every one is remappable via `[keybinds.*]`, see [configuration.md](configuration.md).

| Key | Action |
| --- | --- |
| `↑`/`k`, `↓`/`j` | move cursor in the focused pane |
| `→`/`l`/`enter` | open directory |
| `←`/`h`/`backspace` | go up a directory |
| `space` | mark/unmark item in the focused pane |
| `V` | mark everything between the last mark and the cursor |
| `v` | visual mode; anchors here; moving the cursor marks the whole range, `v`/`esc` exits |
| `d` | download marked/hovered S3 object(s) into the local pane's directory; a single file downloads directly, more than one item (or a single directory) zips |
| `u` | upload marked/hovered local file(s) into the S3 pane's prefix (local pane only) |
| `s` | open the sync dialog |
| `r` | rename; renaming an S3 directory runs as a cancellable background job |
| `O` | open the hovered file in `$EDITOR`/`$VISUAL`; a changed remote file uploads back on exit |
| `y` / `x` | copy / cut marked/hovered item(s) to the paste clipboard |
| `P` | paste the staged clipboard into the focused pane's current location |
| `D` | permanently delete marked/hovered item(s), no undo |
| `Y` | copy the hovered item's `s3://bucket/key` (or local path) to the OS clipboard |
| `U` | copy a temporary, publicly-fetchable share link for the hovered S3 object (1h expiry) |
| `/` | filter the focused pane by name |
| `F1`/`F2`/`F3` | sort the focused pane by name / size / modified (cycles off → asc → desc) |
| `p` | select pane 3's Preview tab, hides the pane if already selected |
| `i` | select pane 3's Info tab, hides the pane if already selected |
| `L` | toggle the local filesystem pane (off by default) |
| `tab` / `shift+tab` | cycle focus forward/backward through local / S3 / preview / jobs |
| `1`-`4` | jump focus directly to local / S3 / preview / jobs |
| `↵`/`l` (jobs focused) | open the selected job's local file/folder with the default app |
| `f` (jobs focused) | reveal the selected job's local file/folder in Finder |
| `c` (jobs focused) | cancel the hovered job |
| `L` (jobs focused) | point the local pane at the job's destination directory |
| `r` (jobs focused) | run the hovered job again |
| `d`/`del`, `C` (jobs focused) | dismiss the hovered finished job / every finished job |
| `esc` | clear the filter, then marks/clipboard |
| `o` | open the bookmark's `web_url` in your browser |
| `E` | events log |
| `t` | toggle light/dark theme |
| `b` | switch bookmark |
| `q` | quit |
| `?` | toggle help |

On the bookmark list: `a` add, `e` edit, `x` delete, `enter` connect.

## Testing

Unit tests sit next to the code they cover (`#[cfg(test)] mod tests` at the bottom of `config/mod.rs`,
`config/keys.rs`, `ui/theme.rs`, `fuzzy.rs`, `local.rs`, `jobs.rs`, `app/mod.rs`) and exercise pure
logic: bookmark path parsing, keybind-spec parsing and override merging, hex-colour parsing, fuzzy
matching, local directory resolution, the cancellable local copy/move helpers behind background
transfers, and the jobs pane's cancel/retry/dismiss rules.

`jobs.rs` also carries a `cancel_tests` module that runs the real download/upload/zip tasks against
a stand-in `StorageProvider` whose transfers hang instead of finishing, so cancelling one is
checked end to end: the job reports `Cancelled`, and a partial file or unfinished archive is
cleaned up rather than left on disk.

Black-box tests under `tests/` drive the same public functions `run_app` calls at startup
(`config::load_app_config_from`, `config::load_connections_from`, `keys::Keybinds::load`) against a
realistic `~/.comhad/`-shaped tempdir, checking the whole config → bookmarks → keybinds pipeline
composes correctly.

Everything that touches disk takes its path as an explicit argument rather than resolving `$HOME`
internally, so tests point it at a tempdir instead of mocking the filesystem. `src/main.rs` is a
two-line shell around `comhad::run_app`; the terminal setup and event loop live in `src/lib.rs`
precisely so the rest of the app doesn't need a real terminal to be testable.
