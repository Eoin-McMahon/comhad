# Configuration

Everything comhad persists lives under `~/.comhad/`:

```
~/.comhad/
├── config.toml       # defaults, theme, keybinds (entirely optional)
└── bookmarks/
    ├── work.json
    └── personal.json
```

Bookmark files created before the `bookmarks/` split existed (directly under `~/.comhad/*.json`)
are moved into place automatically the first time you start comhad after upgrading.

## Bookmarks

One JSON file per connection. Manage them from the app: `a` on the bookmark list to add, `e` to
edit, `x` to delete, or hand-edit:

```json
{
  "name": "bookmark name",
  "profile": "s3",
  "endpoint": "s3.amazonaws.com",
  "access_key_id": "${S3_ACCESS_KEY}",
  "secret_access_key": "${S3_SECRET_KEY}",
  "remote_path": "path-to-bucket",
  "local_path": "~/work/exports",
  "web_url": "https://s3.amazonaws.com"
}
```

Bookmarks written before these fields were renamed (`protocol`, `server`, `path`) still load
unchanged, the old keys are accepted as aliases.

* `profile`: `"s3"` (default) or `"s3_private_link"`. Picks a sane default for
  `force_path_style` (private link endpoints are conventionally virtual-hosted-style).
  (Formerly `protocol`.)
* `endpoint`: bare host or full URL of the S3-compatible endpoint. (Formerly `server`.)
* `force_path_style`: optional override. `true` = `endpoint/bucket/key` (default for
  `profile: s3`, matching Cyberduck's generic S3 profile). `false` = `bucket.endpoint/key`
  (default for `profile: s3_private_link`).
* `remote_path`: `bucket` or `bucket/prefix` to open the browser at. If your credentials can list
  all buckets, comhad shows a bucket picker after connecting instead of pinning you to this one; if
  `s3:ListAllMyBuckets` isn't granted, it falls back to this bucket automatically. (Formerly `path`.)
* `local_path`: optional. The directory the **local** pane opens at for this bookmark, pairing a
  bucket with the directory you sync it against, so `s` is useful the moment you connect rather
  than after navigating there by hand. It's also where `d` downloads land. A leading `~` is
  expanded. Falls back to `[defaults] local_path`, then `~/Downloads`; a directory that doesn't
  exist is skipped with a message rather than opening the pane on nothing.
* `web_url`: optional; opened in your default browser with `o`.
* `region`: optional. If omitted, comhad auto-detects it from an unauthenticated HEAD request's
  `x-amz-bucket-region` header (the same trick Cyberduck and boto3 use) rather than
  `GetBucketLocation`, since that needs its own IAM permission a scoped-down policy often doesn't
  grant. The detected region is then used to build the actual request endpoint
  (`s3.<region>.amazonaws.com`); the global `s3.amazonaws.com` host only transparently serves
  `us-east-1` buckets and returns `PermanentRedirect` for anything else, even when the request is
  correctly signed for that region.

**Credentials.** `access_key_id` and `secret_access_key` can be literal values, or a
`${ENV_VAR_NAME}` reference resolved from your shell environment at startup. comhad only reads
bookmark files at startup to open the S3 client, and writes them back when you use the add/edit
wizard, nothing inspects or transmits them anywhere else. New and edited bookmarks are written
`chmod 600`.

## config.toml

Entirely optional: any section, or the whole file, can be omitted and comhad falls back to its
built-in defaults.

```toml
[defaults]
show_local = false    # local filesystem pane visible at startup
show_preview = true   # preview pane visible at startup

# Where the local pane opens, when the connected bookmark doesn't set its own `local_path`.
# Defaults to ~/Downloads. A leading ~ is expanded. (Formerly `local_dir`, still accepted.)
local_path = "~/work"

[theme]
mode = "light"        # "light" or "dark" at startup; `t` still toggles at runtime

# Optional hex overrides on top of the built-in light/dark palettes; omit any field you
# don't want to change.
[theme.light]
accent = "#c15f42"

[theme.dark]
accent = "#d97757"

# Remap any action's key(s) under [keybinds.<context>]. Comma-separate to bind more than one
# key to an action, e.g. "up,k". Unlisted actions keep their built-in key. See the full list
# of contexts and action names below.
[keybinds.browser]
quit = "q"
toggle_theme = "t"
```

Key-spec syntax: a single case-sensitive character (`"q"`, `"Q"`, `"?"`), a named key (`up`,
`down`, `left`, `right`, `enter`, `esc`, `tab`, `backtab`, `backspace`, `delete`, `space`,
`f1`-`f12`), or a comma-separated list to bind more than one key to the same action (`"up,k"`).

### Keybind contexts and actions

Each table below is a `[keybinds.<context>]` section; the action name is the key on the left,
its built-in default(s) on the right.

#### `connection_picker`

| Action | Default(s) |
| --- | --- |
| `up` | `Up`, `k` |
| `down` | `Down`, `j` |
| `select` | `Enter` |
| `add_bookmark` | `a` |
| `edit_bookmark` | `e` |
| `delete_bookmark` | `x`, `Delete` |
| `toggle_theme` | `t` |
| `help` | `?` |
| `quit` | `q`, `Esc` |

#### `bucket_picker`

| Action | Default(s) |
| --- | --- |
| `up` | `Up`, `k` |
| `down` | `Down`, `j` |
| `select` | `Enter` |
| `toggle_theme` | `t` |
| `back` | `q`, `Esc` |

#### `browser`

| Action | Default(s) |
| --- | --- |
| `quit` | `q` |
| `switch_connection` | `c` |
| `help` | `?` |
| `events` | `E` |
| `toggle_theme` | `t` |
| `preview_tab` | `p` |
| `info_tab` | `i` |
| `toggle_local` | `L` |
| `focus_next` | `Tab` |
| `focus_prev` | `BackTab` |
| `focus_local` | `1` |
| `focus_remote` | `2` |
| `focus_preview` | `3` |
| `focus_transfers` | `4` |
| `sort_name` | `F1` |
| `sort_size` | `F2` |
| `sort_modified` | `F3` |
| `open_web_url` | `o` |
| `reveal_in_finder` | `f` |
| `up` | `Up`, `k` |
| `down` | `Down`, `j` |
| `go_up` | `Left`, `h`, `Backspace` |
| `enter_selected` | `Right`, `l`, `Enter` |
| `toggle_mark` | `space` |
| `toggle_visual` | `v` |
| `download` | `d` |
| `upload` | `u` |
| `open_sync` | `s` |
| `delete` | `D` |
| `stage_copy` | `y` |
| `stage_cut` | `x` |
| `paste` | `P` |
| `copy_location` | `Y` |
| `share_url` | `U` |
| `rename` | `r` |
| `filter` | `/` |
| `cancel` | `Esc` |

#### `help` and `events`

Both are simple scrollable views and share the same two actions.

| Action | Default(s) |
| --- | --- |
| `up` | `Up`, `k` |
| `down` | `Down`, `j` |

#### `sync`

| Action | Default(s) |
| --- | --- |
| `close` | `Esc`, `q` |
| `up` | `Up`, `k` |
| `down` | `Down`, `j` |
| `flip_direction` | `Tab`, `d` |
| `confirm` | `Enter` |

#### `confirm`

The "are you sure?" yes/no dialog shown before download/upload/rename/sync/delete/paste.

| Action | Default(s) |
| --- | --- |
| `toggle_selection` | `Tab`, `BackTab`, `Left`, `Right` |
| `yes` | `y` |
| `no` | `n`, `Esc` |
| `confirm` | `Enter` |

#### `bookmark_delete`

| Action | Default(s) |
| --- | --- |
| `confirm` | `y`, `Enter` |
