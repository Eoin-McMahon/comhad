<p align="center">
<img src="https://raw.githubusercontent.com/Eoin-McMahon/Comhad/master/assets/banner.png?v=2" alt="comhad" style="width:100%;">
</p>

<p align="center">
<a href="https://github.com/Eoin-McMahon/Comhad/actions/workflows/ci.yml"><img src="https://github.com/Eoin-McMahon/Comhad/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
<a href="https://crates.io/crates/comhad"><img src="https://img.shields.io/crates/v/comhad?cacheSeconds=1" alt="crates.io"></a>
<a href="https://crates.io/crates/comhad"><img src="https://img.shields.io/crates/d/comhad?color=d&cacheSeconds=1" alt="downloads"></a>
<a href="https://github.com/Eoin-McMahon/Comhad/blob/master/LICENSE"><img src="https://img.shields.io/github/license/Eoin-McMahon/Comhad" alt="license"></a>
<a href="https://github.com/Eoin-McMahon/Comhad/stargazers"><img src="https://img.shields.io/github/stars/Eoin-McMahon/Comhad" alt="stars"></a>
</p>

*Comhad* (pronounced "KOH-ad") is the Irish word for "file".

## ✨ Features

* **Ranger-style panes**: local ⇄ S3 side by side, with a third pane for preview or object info.
* **Real previews**: syntax-highlighted text, and images inline via Kitty, iTerm2 or Sixel.
* **Background jobs**: every download, upload and zip runs in the background, and the destination's own row fills as the bytes land while the source just spins, so the direction is never ambiguous. Hover a job to cancel, retry, or jump to where it's going.
* **Structure for what can't be previewed**: binary or too-large files show an entropy map of their bytes, so a 2 GB object still tells you whether it's compressed, padded or structured.
* **Prefix composition**: the info tab breaks a directory or prefix down by file type (share, bytes and file count) from the listing it already fetched.
* **Copy, cut and paste**: across directories, across panes, across backends, with ghost rows showing where things will land.
* **Non-destructive sync**: a git-diff-style view of what will be added and updated, in either direction. Sync never deletes.
* **Fuzzy deep filter**: `/` searches the listing and quietly recurses, surfacing matches from nested prefixes.
* **Paired directories**: a bookmark remembers the local directory that goes with its bucket, so sync and downloads point at the right place on connect.
* **Safe by default**: every write confirms first, with the destination spelled out and `No` preselected on delete.

<p align="center">
<img src="https://raw.githubusercontent.com/Eoin-McMahon/Comhad/master/assets/demo.gif?v=3" alt="comhad demo" style="width:100%;">
</p>

## 📦 Installation

#### 🐧 Debian/Ubuntu (apt)

Debian 12 or newer and Ubuntu 22.04 or newer, on amd64 and arm64.

```bash
# Read first: https://github.com/Eoin-McMahon/Comhad/blob/master/docs/apt-repository.md
curl -fsSL https://eoin-mcmahon.github.io/comhad/install.sh | sudo bash
```

That adds the signed apt repository and installs the package, and `apt-get
upgrade` picks up later releases after that. Piping a script into a root shell is
worth checking before you do it: [docs/apt-repository.md](docs/apt-repository.md)
lists the three paths the installer writes, the signing key's fingerprint so you
can verify it, the same steps written out by hand, and how to remove all of it
again. The `.deb` files are also attached to each
[GitHub release](https://github.com/Eoin-McMahon/Comhad/releases) if you would
rather not add the repository at all.

#### 🍺 Homebrew (macOS/Linux)

```bash
brew install eoin-mcmahon/comhad/comhad
```

#### 📥 Install from crates.io

Requires Rust 1.85 or newer ([install Rust](https://www.rust-lang.org/learn/get-started)).

```bash
cargo install comhad
```

#### 🏗️ Build from source

```bash
git clone https://github.com/Eoin-McMahon/Comhad.git
cd Comhad
cargo install --path ./
```

Building on Linux needs the X11 clipboard headers: `sudo apt install libxcb1-dev
libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev`. This applies only when
compiling; the apt package needs nothing beyond a base system.

## 🚀 Quick start

```bash
comhad
```

Press `a` on the bookmark list to add a connection, fill in the wizard, and `enter` to connect.
That's the whole setup: no config file required. If you want one, for startup defaults, theme
colours or custom keybindings, you can add `~/.comhad/config.toml` yourself (see the
Configuration section below), but it's entirely optional.

## ⌨️ Keys

| Key | Action |
| --- | --- |
| `hjkl` / arrows | navigate; `l`/`enter` opens, `h` goes up |
| `space` / `v` | mark item / visual-mode range select |
| `d` / `u` | download / upload marked items |
| `y` `x` `P` | copy / cut / paste, in any direction |
| `D` / `r` | delete (no undo) / rename |
| `O` | open in `$EDITOR` (remote edits upload back on change) |
| `s` | sync dialog |
| `/` | fuzzy filter, with recursive deep matches |
| `p` / `i` | preview pane / info pane |
| `L` / `1`-`4` | show the local pane / jump to a pane |
| `t` `E` `?` `q` | theme / events log / help / quit |

The mouse works too: click a row to select it (click again to open), scroll to move the cursor,
click a breadcrumb segment in the S3 pane's title to jump there. Toggle off with `[ui] mouse =
false` if you'd rather keep the terminal's native click-drag text selection unmodified.

[Full keybindings and what each feature does →](docs/usage.md)

## ⚙️ Configuration

Bookmarks live in `~/.comhad/bookmarks/*.json`, one per connection, and can be managed entirely
from the app. Everything else is optional: `~/.comhad/config.toml` sets startup defaults, theme
colours and keybindings, and comhad works fine without it; omit any section, or the whole file.

Every option below is shown at its built-in default, so this is also a valid starting point to
copy and edit:

```toml
[defaults]
show_local = false    # local filesystem pane visible at startup: true · false
show_preview = true   # preview pane visible at startup: true · false

# Where the local pane opens when the connected bookmark doesn't set its own `local_path`.
# A leading ~ is expanded. Unset (the default) falls back to ~/Downloads.
# local_path = "~/work"

[theme]
mode = "light"   # startup theme: "light" · "dark" (`t` still toggles either way at runtime)

# Optional hex overrides on top of the built-in palette for each mode. Every field below is
# unset by default (i.e. uses the built-in colour); only override the ones you want to change.
[theme.light]
# bg         = "#ffffff"   # app background
# panel_bg   = "#f4f4f4"   # pane background
# accent     = "#0031a9"   # borders, titles, selection highlight
# accent_dim = "#8a8fa8"   # dim accent (inactive pane borders, hints)
# on_accent  = "#ffffff"   # text drawn on top of an accent-coloured background
# text       = "#1a1a1a"   # primary text
# muted      = "#767676"   # secondary text (sizes, timestamps, hints)
# good       = "#1a7f37"   # success/add colour (sync adds, copy staging, uploads)
# bad        = "#c00f0c"   # error/danger colour (delete, cut staging, failures)
# dir        = "#0031a9"   # directory name colour

[theme.dark]
# Same fields as [theme.light]; comhad's dark-mode built-ins apply if left unset.
# accent = "#2fafff"

[ui]
# File/directory icons: "auto" (default) probes common font directories for an installed Nerd
# Font (nerdfonts.com) and uses it if found, else falls back to plain Unicode/emoji. The probe
# can't see which font your terminal actually renders with, so a wrong guess is possible. Set
# this explicitly to skip it. "nerdfont" needs a Nerd Font installed *and* selected as your
# terminal's font (each file type then gets its own logo + colour, nvim-web-devicons style), or
# icons render as boxes. Possible values: "auto" · "nerdfont" · "unicode"
icons = "auto"

# Click a row to select it (a second click opens it), scroll to move the cursor, click a remote
# breadcrumb segment to jump there. Capturing the mouse takes over the terminal's native
# click-drag text selection (most terminals still let you select text while holding a modifier,
# e.g. Option on iTerm2/macOS Terminal, Shift on many Linux terminals). Possible values: true · false
mouse = true

# Remap any action's key(s) under [keybinds.<context>]. Comma-separate to bind more than one key
# to an action, e.g. "up,k". Unlisted actions keep their built-in default. Contexts:
# connection_picker, bucket_picker, browser, jobs, help, events, sync, confirm, bookmark_delete.
# Full action list per context, with defaults → docs/configuration.md.
[keybinds.browser]
# quit = "q"
# toggle_theme = "t"
```

[Full configuration reference (every keybind context/action, bookmark fields) →](docs/configuration.md)

### 🎨 Icons

A [Nerd Font](https://www.nerdfonts.com/) is recommended: with one installed and selected as
your terminal font, each file type gets its own coloured logo (the nvim-web-devicons palette).
Without one, comhad falls back to plain emoji icons, so it still works out of the box. The `auto`
probe can't tell which font your terminal actually renders with, so if the guess is wrong, set
`[ui] icons` explicitly (see the config example above).

## 🧑‍💻 Development

```bash
cargo build --release
cargo test
cargo clippy --all-targets
```

Storage backends sit behind a single `StorageProvider` trait in `src/provider/`, so adding another
service is a matter of implementing that trait, the most useful place to start if you'd like to
contribute. Issues and pull requests welcome.

## 📄 License

[MIT](LICENSE) © Eóin McMahon

<sub><i>comhad</i> (pronounced "KOH-ad") is the Irish word for "file".</sub>
