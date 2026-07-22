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
  "protocol": "s3",
  "server": "s3.amazonaws.com",
  "access_key_id": "${S3_ACCESS_KEY}",
  "secret_access_key": "${S3_SECRET_KEY}",
  "path": "path-to-bucket",
  "local_path": "~/work/exports",
  "web_url": "https://s3.amazonaws.com"
}
```

* `protocol`: `"s3"` (default) or `"s3_private_link"`. Picks a sane default for
  `force_path_style` (private link endpoints are conventionally virtual-hosted-style).
* `server`: bare host or full URL of the S3-compatible endpoint.
* `force_path_style`: optional override. `true` = `endpoint/bucket/key` (default for
  `protocol: s3`, matching Cyberduck's generic S3 profile). `false` = `bucket.endpoint/key`
  (default for `protocol: s3_private_link`).
* `path`: `bucket` or `bucket/prefix` to open the browser at. If your credentials can list all
  buckets, comhad shows a bucket picker after connecting instead of pinning you to this one; if
  `s3:ListAllMyBuckets` isn't granted, it falls back to this bucket automatically.
* `local_path`: optional. The directory the **local** pane opens at for this bookmark, pairing a
  bucket with the directory you sync it against, so `s` is useful the moment you connect rather
  than after navigating there by hand. It's also where `d` downloads land. A leading `~` is
  expanded. Falls back to `[defaults] local_dir`, then `~/Downloads`; a directory that doesn't
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
# Defaults to ~/Downloads. A leading ~ is expanded.
local_dir = "~/work"

[theme]
mode = "light"        # "light" or "dark" at startup; `t` still toggles at runtime

# Optional hex overrides on top of the built-in light/dark palettes; omit any field you
# don't want to change.
[theme.light]
accent = "#c15f42"

[theme.dark]
accent = "#d97757"

# Remap any action's key(s). Comma-separate to bind more than one key to an action, e.g.
# "up,k". Unlisted actions keep their built-in key. See src/keys.rs for the full list of
# action names per context (connection_picker, bucket_picker, browser, help, events, sync,
# confirm, bookmark_delete).
[keybinds.browser]
quit = "q"
toggle_theme = "t"
```
