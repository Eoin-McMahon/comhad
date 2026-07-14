# Comhad

A pretty ranger-style terminal S3 browser with a multi-pane layout.

comhad can rename, upload, download, sync, copy, move, and delete. Sync stays one-way and
non-destructive (it never deletes anything the destination has extra), and every one of
these — including delete — asks "are you sure?" first; there's no undo once you confirm.

Storage backends live behind a single `StorageProvider` trait (`src/provider/`), so S3 is
just the first implementation — adding another service (GCS, Dropbox, …) is a matter of
implementing that trait for a new type.

## Configuration

Everything comhad persists lives under `~/.comhad/`:

```
~/.comhad/
├── config.toml       # defaults, theme, keybinds — see below (optional; comhad works with none of this)
└── bookmarks/
    ├── work.json
    └── personal.json
```

Bookmark files created before this split existed (directly under `~/.comhad/*.json`) are
moved into `bookmarks/` automatically the first time you start comhad after upgrading.

`config.toml` is entirely optional — any section, or the whole file, can be omitted and
comhad falls back to its built-in defaults:

```toml
[defaults]
show_local = false    # local filesystem pane visible at startup
show_preview = true   # preview pane visible at startup

[theme]
mode = "light"        # "light" or "dark" at startup; `t` still toggles at runtime

# Optional hex overrides on top of the built-in tinted Modus Operandi (light) / tinted
# Modus Vivendi (dark) palettes — omit any field you don't want to change.
[theme.light]
accent = "#0031a9"

[theme.dark]
accent = "#2fafff"

# Remap any action's key(s). Comma-separate to bind more than one key to an action, e.g.
# "up,k". Unlisted actions keep their built-in key. See src/keys.rs for the full list of
# action names per context (connection_picker, bucket_picker, browser, help, events, sync,
# confirm, bookmark_delete).
[keybinds.browser]
quit = "q"
toggle_theme = "t"
```

## Bookmarks

comhad reads one JSON file per bookmark from `~/.comhad/bookmarks/*.json`. You can manage
them entirely from the app, press `a` on the bookmark list to add one, `e` to edit, `x` to
delete, or hand-edit the JSON directly:

```json
{
  "name": "bookmark name",
  "protocol": "s3",
  "server": "s3.amazonaws.com",
  "access_key_id": "${S3_ACCESS_KEY}",
  "secret_access_key": "${S3_SECRET_KEY}",
  "path": "path-to-bucket",
  "web_url": "https://s3.amazonaws.com"
}
```

- `protocol`, `"s3"` (default) or `"s3_private_link"`. Picks a sane default for
  `force_path_style` (private link endpoints are conventionally virtual-hosted-style).
- `server`, bare host or full URL of the S3-compatible endpoint.
- `force_path_style`, optional override. `true` = `endpoint/bucket/key` (default for
  `protocol: s3`, matching Cyberduck's generic S3 profile). `false` = `bucket.endpoint/key`
  (default for `protocol: s3_private_link`).
- `path`, `bucket` or `bucket/prefix` to open the browser at. If your credentials can list
  all buckets, comhad shows a bucket picker after connecting instead of pinning you to this
  one; if `s3:ListAllMyBuckets` isn't granted, it falls back to this bucket automatically.
- `web_url`, optional; opened in your default browser with `o`.
- `region`, optional. If omitted, comhad auto-detects it from an unauthenticated HEAD
  request's `x-amz-bucket-region` header (the same trick Cyberduck/boto3 use) rather than
  `GetBucketLocation`, since that needs its own IAM permission a scoped-down policy often
  doesn't grant. The detected region is then used to build the actual request endpoint
  (`s3.<region>.amazonaws.com`), the global `s3.amazonaws.com` host only transparently
  serves `us-east-1` buckets and returns `PermanentRedirect` for anything else, even when
  the request is correctly signed for that region.

**Credentials**: `access_key_id` and `secret_access_key` can be literal values, or a
`${ENV_VAR_NAME}` reference resolved from your shell environment at startup.
Comhad only ever reads bookmark files at startup to open the S3 client and writes them
back out when you use the add/edit wizard. nothing in the tool inspects or transmits 
them elsewhere. New/edited bookmarks are written
`chmod 600` (owner read/write only).

## Layout

The S3 pane is the main view. A **preview** pane sits alongside it (`p` opens it, or hides it
again if it's already on the Preview tab). A transfers strip along the bottom shows live
progress for every download/upload/zip job for the session, tab (or `4`) into it to browse
past transfers and open or reveal a finished one in Finder rather than hunting for it in the
local pane.

A **local filesystem** pane is available for browsing to a file to upload without
drag-and-drop or typing a path, but it's off by default, press `L` to bring it in (splits
into a three-column local/S3/preview layout) and `tab` to switch focus between it and the S3
pane.

Preview skips anything over 5 MB (and shows "file too large to preview" instead) so a huge
object or video file never adds lag. Text/config files are read as a small, bounded snippet
(4 KB) rather than the whole file, and recognized source/config types are syntax-highlighted
(`syntect`) off the render loop so it never causes lag. Images (`png`/`jpg`/`gif`/`bmp`/`webp`,
still capped at 5 MB) render inline via `ratatui-image`, using whatever graphics protocol your
terminal supports (Kitty, iTerm2, Sixel) and falling back to halfblock ASCII-art otherwise.

Pane 3 is actually two tabs, both labeled at the top of the pane: **Preview** (`p`) and
**Info** (`i`). Info shows name, key/path, size, last-modified, and (for a remote object)
whatever metadata the backend returns (S3 gives ETag, Content-Type, and Storage Class; the
field list is generic, so a future non-S3 backend can return entirely different metadata
without any UI changes). Switching tabs doesn't depend on what's previewable — if a file's
too large or binary, `p` still shows that message, and `i` still gets you its info instead.
Pressing the key for whichever tab is already active hides the pane, same as `p` always did;
pressing the other one just switches tabs without hiding anything.

## Confirmations and events

Every write action (download, upload, rename, sync, delete, paste) asks "are you sure?" with
a destination/source path called out on its own highlighted line, and two tabbed **Yes**/**No**
buttons at the bottom — `tab`/arrows flip which one's selected, `enter` activates it, or just
press `y`/`n`/`esc` directly. Delete starts with **No** selected, since it's the one action
with no undo; everything else starts on **Yes**.

The footer shows the most recent status message (green for success, red for failure) for a
few seconds, then clears itself. Press `E` any time to see the full events log for the
session — every status message, newest first, with the complete error chain and connection
diagnostics kept under any failure rather than just the one-line summary.

## Move, copy, and delete

Mark items (`space`) or just hover one, then:

- `y` copies them to a clipboard, `x` cuts them — either way, navigate anywhere (same pane, a
  different directory, the other pane entirely) and press `P` to paste. Staged items render in
  a distinct color (green `⧉` for copy, red `✂` for cut) so it's obvious what's queued. Pasting
  works within local, within S3, and in either direction between them; a cross-backend move
  transfers the file first and only removes the source once that transfer actually succeeds.
  While anything's staged, every pane you navigate to grows a greyed, italic `+⧉`/`+✂` ghost row
  per staged item, previewing where it'll land before you ever press `P`.
- `D` permanently deletes the marked/hovered item(s) — no undo.
- `Y` copies the hovered item's `s3://bucket/key` (or local absolute path) to your OS clipboard.

Every one of these confirms first. Pasting (any direction) runs as a background transfer job,
same as upload/download — shows up in the transfers strip with a spinner while running, `→`
once a same-store copy/move lands, and `esc` cancels whatever's currently running.

## Deep filter

`/` filters the currently listed directory by name — fuzzy (characters just need to appear in
order, not contiguously, so `hlo` matches `hello.csv`), instant, since it's just filtering
what's already loaded, and matched characters are highlighted in the name. The first time you
type a non-empty filter, it also kicks off
a one-time recursive scan under the current prefix/directory (cached for the rest of that
filter session, so every keystroke after that is still just an in-memory re-filter). Any match
found elsewhere gets appended below the normal listing, showing its relative path and a
distinct color, rather than only ever surfacing whichever copy happens to be a direct child —
so filtering for `hello.csv` with one copy at the root and another under `archive/2024/` shows
both. `enter` on one of these appended rows jumps you to its actual location; everything else
(marking, download, rename, `y`/`x`/`P`, `D`) already works on them directly too, since they
carry their real key/path.

## Sync

Press `s` to open the sync dialog. It diffs the local pane's directory against the S3 pane's
prefix and shows every file with a git-diff status icon, colored by what will actually happen:
**`+` green** to add (missing on the destination), **`~` amber** to update (present but a
different size or the source is newer), and **grey** for no-ops — both **`=`** unchanged files
and **`-`** files that exist only on the destination (shown for awareness, but skipped: sync
never deletes). An add also projects onto the destination panel in green, so you can see the
file appear on the side it's about to land on. `tab`/`d` flips the direction (local→remote
upload ⇄ remote→local download) and rescans; `enter` runs it as normal transfer jobs; `esc`
closes.

## Keybindings

These are the built-in defaults; every one is remappable via `[keybinds.*]` in
`~/.comhad/config.toml` (see "Configuration" above).

| Key | Action |
| --- | --- |
| `↑`/`k`, `↓`/`j` | move cursor in the focused pane |
| `→`/`l`/`enter` | open directory |
| `←`/`h`/`backspace` | go up a directory |
| `space` | mark/unmark item in the focused pane |
| `v` | visual mode — anchors here; moving the cursor marks the whole range (vim-style), `v`/`esc` exits |
| `d` | download marked/hovered S3 object(s) into the local pane's directory (S3 pane only) — a single file downloads directly, more than one item (or a single directory) zips |
| `u` | upload marked/hovered local file(s) into the S3 pane's prefix (local pane only, needs `L` on) |
| `s` | open the sync dialog (diff local ⇄ remote, transfer missing/newer, never delete) |
| `r` | rename; renaming an S3 directory runs as a cancellable background job |
| `y` / `x` | copy / cut marked/hovered item(s) to the paste clipboard |
| `P` | paste the staged clipboard into the focused pane's current location |
| `D` | permanently delete marked/hovered item(s) — no undo; an S3 directory deletes as a cancellable background job |
| `Y` | copy the hovered item's `s3://bucket/key` (or local path) to the OS clipboard |
| `U` | copy a temporary, publicly-fetchable share link for the hovered S3 object (1h expiry) to the OS clipboard |
| `/` | filter the focused pane (local or S3) by name — see "Deep filter" below |
| `F1`/`F2`/`F3` | sort the focused pane by name / size / modified (cycles off → asc → desc) |
| `p` | select pane 3's Preview tab (file content) — hides the pane if already selected |
| `i` | select pane 3's Info tab (name/key/size/ETag/etc) — hides the pane if already selected |
| `L` | toggle the local filesystem pane (off by default) |
| `tab` / `shift+tab` | cycle focus forward/backward through local / S3 / preview / transfers |
| `1`-`4` | jump focus directly to local / S3 / preview / transfers |
| `↵`/`l` (transfers focused) | open the selected transfer's local file/folder with the default app |
| `f` (transfers focused) | reveal the selected transfer's local file/folder in Finder |
| `esc` | cancel every currently-running transfer; otherwise clear the filter, then marks/clipboard |
| `o` | open the bookmark's `web_url` in your browser |
| `E` | events log — every status message this session, newest first, with full detail under errors |
| `t` | toggle light/dark theme (starts in light) |
| `c` | switch bookmark |
| `q` | quit |
| `?` | toggle help |

On the bookmark list: `a` add, `e` edit, `x` delete, `enter` connect.

Dropping a file from Finder onto the terminal window still works regardless of whether `L`
is on, comhad catches it via bracketed paste and offers to upload it into the S3 pane's
current prefix.

## Build

```bash
cargo build --release
```

## Testing

```bash
cargo test
```

Unit tests sit next to the code they cover (`#[cfg(test)] mod tests` at the bottom of
`config.rs`, `keys.rs`, `ui/theme.rs`, `fuzzy.rs`) and exercise pure logic — bookmark path
parsing, keybind-spec parsing and override merging, hex-color parsing, fuzzy matching.

A couple of black-box tests under `tests/` drive the same public functions `run_app` calls
at startup (`config::load_app_config_from`, `config::load_connections_from`,
`keys::Keybinds::load`) against a realistic `~/.comhad/`-shaped tempdir, checking the whole
config → bookmarks → keybinds pipeline composes correctly.

Everything that touches disk takes its directory/file path as an explicit argument rather
than resolving `$HOME` internally, so tests point it at a tempdir instead of mocking the
filesystem. `src/main.rs` itself is a two-line shell around `comhad::run_app` — the actual
terminal setup/event loop lives in `src/lib.rs` precisely so the rest of the app doesn't need
a real terminal to be testable.
