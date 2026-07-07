# Comhad

A pretty ranger-style terminal S3 browser with a multi-pane layout.

comhad's only mutating S3 actions are **rename** and **upload**, there is no delete.
Renaming a "folder" internally recopies every object under it and removes the old keys, but
that's the sole place an object is ever removed from a bucket.

## Bookmarks

comhad reads one JSON file per bookmark from `~/.comhad/*.json`. You can manage them
entirely from the app, press `a` on the bookmark list to add one, `e` to edit, `x` to
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

The S3 pane is the main view. A **preview** of whatever's under the cursor sits alongside it
(toggle with `p`). A transfers strip along the bottom shows live progress for every
download/upload/zip job for the session, tab (or `4`) into it to browse past transfers and
open or reveal a finished one in Finder rather than hunting for it in the local pane.

A **local filesystem** pane is available for browsing to a file to upload without
drag-and-drop or typing a path, but it's off by default, press `L` to bring it in (splits
into a three-column local/S3/preview layout) and `tab` to switch focus between it and the S3
pane.

Preview skips anything over 5 MB (and shows "file too large to preview" instead) so a huge
object or video file never adds lag, everything else is read as a small, bounded snippet
(4 KB), not the whole file.

## Keybindings

| Key | Action |
| --- | --- |
| `↑`/`k`, `↓`/`j` | move cursor in the focused pane |
| `→`/`l`/`enter` | open directory |
| `←`/`h`/`backspace` | go up a directory |
| `space` | mark/unmark item in the focused pane |
| `d` | download marked/hovered S3 object(s) into the local pane's current directory |
| `u` | upload marked/hovered local file(s) into the S3 pane's current prefix (needs `L` on) |
| `r` | rename |
| `/` | filter the S3 pane |
| `p` | toggle the preview pane |
| `L` | toggle the local filesystem pane (off by default) |
| `tab` / `shift+tab` | cycle focus forward/backward through local / S3 / preview / transfers |
| `1`-`4` | jump focus directly to local / S3 / preview / transfers |
| `↵`/`l` (transfers focused) | open the selected transfer's local file/folder with the default app |
| `f` (transfers focused) | reveal the selected transfer's local file/folder in Finder |
| `o` | open the bookmark's `web_url` in your browser |
| `E` | show full error details after a failure (short message alone often isn't enough) |
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
