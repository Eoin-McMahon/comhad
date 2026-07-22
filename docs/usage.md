# Usage

Everything comhad does, and the keys that do it. See [configuration.md](configuration.md) for
bookmarks, theming and remapping keys.

## The layout

The S3 pane is the main view. A **preview** pane sits alongside it (`p` opens it, or hides it again
if it's already on the Preview tab). A transfers strip along the bottom shows live progress for
every download/upload/zip job for the session — `tab`, or `4`, into it to browse past transfers and
open or reveal a finished one in Finder rather than hunting for it in the local pane.

A **local filesystem** pane is available for browsing to a file to upload without drag-and-drop or
typing a path, but it's off by default. Press `L` to bring it in (splitting into a three-column
local/S3/preview layout) and `tab` to switch focus between it and the S3 pane.

Dropping a file from Finder onto the terminal window works regardless of whether `L` is on — comhad
catches it via bracketed paste and offers to upload it into the S3 pane's current prefix.

Pane 3 is two tabs, both labelled at the top: **Preview** (`p`) and **Info** (`i`). Info shows name,
key/path, size, last-modified, and whatever metadata the backend returns — S3 gives ETag,
Content-Type and Storage Class, but the field list is generic, so a future non-S3 backend can return
entirely different metadata without any UI changes. Switching tabs doesn't depend on what's
previewable: if a file's too large or binary, `p` still says so and `i` still gets you its info.

Preview skips anything over 5 MB (showing "file too large to preview") so a huge object never adds
lag. Text and config files are read as a bounded 4 KB snippet rather than the whole file, and
recognised source types are syntax-highlighted off the render loop. Images (`png`/`jpg`/`gif`/`bmp`/
`webp`, same 5 MB cap) render inline via `ratatui-image`, using whatever graphics protocol your
terminal supports (Kitty, iTerm2, Sixel) and falling back to halfblock ASCII otherwise.

## Move, copy, and delete

Mark items (`space`) or just hover one, then:

* `y` copies them to a clipboard, `x` cuts them — either way, navigate anywhere (same pane, a
  different directory, the other pane entirely) and press `P` to paste. Staged items render in a
  distinct colour (green `⧉` for copy, red `✂` for cut) so it's obvious what's queued. Pasting works
  within local, within S3, and in either direction between them; a cross-backend move transfers the
  file first and only removes the source once that transfer actually succeeds. While anything's
  staged, every pane you navigate to grows a greyed, italic `+⧉`/`+✂` ghost row per staged item,
  previewing where it'll land before you ever press `P`.
* `D` permanently deletes the marked/hovered item(s) — no undo.
* `Y` copies the hovered item's `s3://bucket/key` (or local absolute path) to your OS clipboard.

Every one of these confirms first. Pasting runs as a background transfer job, same as
upload/download — it shows up in the transfers strip with a spinner while running, `→` once a
same-store copy/move lands, and `esc` cancels whatever's currently running.

## Sync

Press `s` to open the sync dialog. It diffs the local pane's directory against the S3 pane's prefix
and shows every file with a git-diff status icon, coloured by what will actually happen: **`+` green**
to add (missing on the destination), **`~` amber** to update (present but a different size, or the
source is newer), and **grey** for no-ops — both **`=`** unchanged files and **`-`** files that exist
only on the destination (shown for awareness, but skipped: sync never deletes). An add also projects
onto the destination panel in green, so you can see the file appear on the side it's about to land on.

`tab`/`d` flips the direction (local→remote upload ⇄ remote→local download) and rescans, `enter` runs
it as normal transfer jobs, and `esc` closes.

Give a bookmark a `local_path` and the local pane opens on the directory that bucket pairs with, so
`s` diffs the right two trees straight after connecting.

## Deep filter

`/` filters the current directory by name — fuzzy (characters just need to appear in order, not
contiguously, so `hlo` matches `hello.csv`) and instant, since it's only filtering what's already
loaded, with matched characters highlighted.

The first non-empty filter also kicks off a one-time recursive scan under the current prefix, cached
for the rest of that filter session, so every keystroke after that is still an in-memory re-filter.
Any match found elsewhere is appended below the normal listing with its relative path in a distinct
colour, rather than only ever surfacing whichever copy happens to be a direct child — so filtering
for `hello.csv` with one copy at the root and another under `archive/2024/` shows both. `enter` on
one of those rows jumps to its real location, and everything else (marking, download, rename,
`y`/`x`/`P`, `D`) works on them directly, since they carry their real key/path.

## Confirmations and events

Every write action asks "are you sure?" with the destination/source path on its own highlighted line
and **Yes**/**No** buttons — `tab`/arrows flip which is selected, `enter` activates it, or press
`y`/`n`/`esc` directly. Delete starts on **No**, since it's the one action with no undo; everything
else starts on **Yes**.

The footer shows the most recent status message (green for success, red for failure) for a few
seconds. Press `E` for the full events log for the session — every status message, newest first, with
the complete error chain and connection diagnostics kept under any failure rather than just the
one-line summary.

## Keybindings

Defaults; every one is remappable via `[keybinds.*]` — see [configuration.md](configuration.md).

| Key | Action |
| --- | --- |
| `↑`/`k`, `↓`/`j` | move cursor in the focused pane |
| `→`/`l`/`enter` | open directory |
| `←`/`h`/`backspace` | go up a directory |
| `space` | mark/unmark item in the focused pane |
| `v` | visual mode — anchors here; moving the cursor marks the whole range, `v`/`esc` exits |
| `d` | download marked/hovered S3 object(s) into the local pane's directory — a single file downloads directly, more than one item (or a single directory) zips |
| `u` | upload marked/hovered local file(s) into the S3 pane's prefix (local pane only) |
| `s` | open the sync dialog |
| `r` | rename; renaming an S3 directory runs as a cancellable background job |
| `y` / `x` | copy / cut marked/hovered item(s) to the paste clipboard |
| `P` | paste the staged clipboard into the focused pane's current location |
| `D` | permanently delete marked/hovered item(s) — no undo |
| `Y` | copy the hovered item's `s3://bucket/key` (or local path) to the OS clipboard |
| `U` | copy a temporary, publicly-fetchable share link for the hovered S3 object (1h expiry) |
| `/` | filter the focused pane by name |
| `F1`/`F2`/`F3` | sort the focused pane by name / size / modified (cycles off → asc → desc) |
| `p` | select pane 3's Preview tab — hides the pane if already selected |
| `i` | select pane 3's Info tab — hides the pane if already selected |
| `L` | toggle the local filesystem pane (off by default) |
| `tab` / `shift+tab` | cycle focus forward/backward through local / S3 / preview / transfers |
| `1`-`4` | jump focus directly to local / S3 / preview / transfers |
| `↵`/`l` (transfers focused) | open the selected transfer's local file/folder with the default app |
| `f` (transfers focused) | reveal the selected transfer's local file/folder in Finder |
| `esc` | cancel every running transfer; otherwise clear the filter, then marks/clipboard |
| `o` | open the bookmark's `web_url` in your browser |
| `E` | events log |
| `t` | toggle light/dark theme |
| `c` | switch bookmark |
| `q` | quit |
| `?` | toggle help |

On the bookmark list: `a` add, `e` edit, `x` delete, `enter` connect.

## Testing

Unit tests sit next to the code they cover (`#[cfg(test)] mod tests` at the bottom of `config.rs`,
`keys.rs`, `ui/theme.rs`, `fuzzy.rs`, `local.rs`) and exercise pure logic — bookmark path parsing,
keybind-spec parsing and override merging, hex-colour parsing, fuzzy matching, local directory
resolution.

Black-box tests under `tests/` drive the same public functions `run_app` calls at startup
(`config::load_app_config_from`, `config::load_connections_from`, `keys::Keybinds::load`) against a
realistic `~/.comhad/`-shaped tempdir, checking the whole config → bookmarks → keybinds pipeline
composes correctly.

Everything that touches disk takes its path as an explicit argument rather than resolving `$HOME`
internally, so tests point it at a tempdir instead of mocking the filesystem. `src/main.rs` is a
two-line shell around `comhad::run_app` — the terminal setup and event loop live in `src/lib.rs`
precisely so the rest of the app doesn't need a real terminal to be testable.
