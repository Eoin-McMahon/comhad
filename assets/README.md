# Assets

The README links to the PNGs by raw GitHub URL, so they must keep these names and live on `master`.
The SVGs are the sources — edit those and re-render, don't touch the PNGs directly.

| File | What it is | Rebuild with |
| --- | --- | --- |
| `logo.svg` → `logo.png` | The mark: the Dagda's cauldron, files coming back out | `rsvg-convert -w 512 -h 512 assets/logo.svg -o assets/logo.png` |
| `banner.svg` → `banner.png` | README header — mark, wordmark, tagline | `rsvg-convert -w 1600 -h 620 assets/banner.svg -o assets/banner.png` |
| `demo.gif` | The recording under the badges | `./assets/demo/record.sh` |

`rsvg-convert` comes from librsvg (`brew install librsvg`).

## The mark

The [Dagda's cauldron](https://en.wikipedia.org/wiki/Coire_Ansic) — one of the Four Treasures of
Ireland, the vessel from which no company ever went away unsatisfied. A bottomless store, which is
what an object store is, and it suits a tool named for the Irish word for "file".

Colours are the app's own, from `src/ui/theme.rs`: `#4c7a36` green, `#c15f42` terracotta, `#faf7f2`
cream. The banner sits on `#f4e9e3`, a light terracotta tint.

The banner's wordmark uses Futura Bold and the tagline CaskaydiaCove Nerd Font, both resolved by
fontconfig at render time — on a machine without them, rsvg falls back and the banner will look
different, so re-render on macOS or adjust the `font-family` lists.

`assets/` is excluded from the published crate (see `exclude` in `Cargo.toml`), so none of this
bloats a `cargo install`.

## Re-recording the demo

```bash
./assets/demo/record.sh
```

Spins up a throwaway localstack (port 24566, its own container, loopback only), seeds it, records
`demo.tape` with [VHS](https://github.com/charmbracelet/vhs), optimises the GIF with gifsicle, and
tears the container down. Needs `vhs`, `gifsicle` and a running Docker.

Nothing in it touches your real `~/.comhad`, your AWS credentials, or any other localstack — the
recording runs under `HOME=/tmp/comhad-demo`, and `record.sh` refuses to start if something already
holds the port.

### Notes for editing the tape

- Cursor counts in `demo.tape` depend on the exact listing `seed.sh` produces. Change the seed data
  and the `j`/`k` counts need revisiting. `h` returns the cursor to the first row.
- The preview pane is open from startup — pressing `p` on the Preview tab *hides* it.
- **VHS cannot send function keys**, so sorting (`F1`/`F2`/`F3`) can't be demonstrated; the help
  screen at least documents it on camera.
- **Image previews don't render under VHS** — it draws through headless chromium, which supports no
  terminal graphics protocol, so the pane comes out blank rather than falling back to halfblocks.
