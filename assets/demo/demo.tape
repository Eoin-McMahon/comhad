# Hero GIF for the README. Regenerate with: ./assets/demo/record.sh
#
# Cursor counts assume the bucket and local dirs seeded by seed.sh + seed-local.sh:
#   root:          archive config data images reports src  pipeline.yaml README.md summary.csv
#   src/:          ingest.py schema.sql transform.rs
#   data/:         2024-01-15 2024-01-16 exports
#   data/exports/: customers.csv orders.csv summary.csv
#   work/exports/: orders.csv q1-forecast.csv customers.csv   (local pane, newest first)
#   bookmarks, in load order: "exports sync (localstack)", "demo (localstack)"
#
# `h` returns the cursor to the first row, so counts after it start from the top.
#
# Not shown: sorting (F1/F2/F3 — VHS cannot send function keys) and image previews (VHS
# renders through headless chromium, which draws no terminal graphics protocol).

Output "assets/demo.gif"

Require comhad

Set Shell "zsh"
Set FontFamily "CaskaydiaCove Nerd Font"
Set FontSize 16
Set Width 1280
Set Height 640
Set Padding 0
Set Framerate 20
Set TypingSpeed 85ms
Set WindowBar Colorful
Set BorderRadius 10
Set Theme { "name": "comhad", "background": "#faf7f2", "foreground": "#2b2722", "cursor": "#c15f42", "selection": "#d9b3a4", "black": "#2b2722", "red": "#b83a2b", "green": "#4c7a36", "yellow": "#a36a14", "blue": "#3a6ea5", "magenta": "#8a5340", "cyan": "#4c7a7a", "white": "#faf7f2", "brightBlack": "#7a7267", "brightRed": "#c15f42", "brightGreen": "#4c7a36", "brightYellow": "#a36a14", "brightBlue": "#3a6ea5", "brightMagenta": "#8a5340", "brightCyan": "#4c7a7a", "brightWhite": "#ffffff" }

# ── setup, hidden ──────────────────────────────────────────────────
Hide
Type "export PS1='%F{242}~%f ❯ '"
Enter
Type "clear"
Enter
Show

# ══ launch ═════════════════════════════════════════════════════════
Sleep 600ms
Type "comhad"
Sleep 350ms
Enter
Sleep 1600ms
Sleep 700ms
Type "j"
Sleep 450ms
Enter
Sleep 1400ms
Enter
Sleep 1800ms

# ══ previews follow the cursor ═════════════════════════════════════
Type@240ms "jjjjj"
Sleep 300ms
Type@240ms "jjj"
Sleep 350ms
Type "k"
Sleep 2100ms
Type "k"
Sleep 1800ms

# JSON
Type@200ms "kkkkk"
Sleep 450ms
Enter
Sleep 2800ms

# Python, SQL, Rust
Type "h"
Sleep 800ms
Type@220ms "jjjjj"
Sleep 400ms
Enter
Sleep 2900ms
Type "j"
Sleep 1700ms
Type "j"
Sleep 1700ms
Type "h"
Sleep 800ms

# ══ the local pane, and jumping between the four panes ═════════════
Type "L"
Sleep 2800ms
Type "1"
Sleep 1800ms
Type "3"
Sleep 1500ms
Type "4"
Sleep 1500ms
Type "2"
Sleep 1200ms

# ══ download one file, then look at where it landed ════════════════
Enter
Sleep 1200ms
Enter
Sleep 1200ms
Type "d"
Sleep 1800ms
Enter
Sleep 2600ms
# focus the local pane and put the cursor on the file that just landed (5 dirs come first)
Type "1"
Sleep 1000ms
Type@200ms "jjjjj"
Sleep 3000ms
Type "2"
Sleep 900ms

# ══ download several at once (more than one item zips) ═════════════
Type "h"
Sleep 700ms
Type "h"
Sleep 900ms
Type@220ms "jjjjj"
Sleep 400ms
Enter
Sleep 1400ms
Space
Sleep 400ms
Type "j"
Sleep 300ms
Space
Sleep 400ms
Type "j"
Sleep 300ms
Space
Sleep 900ms
Type "d"
Sleep 2000ms
Enter
Sleep 3000ms
# the zip is the newest file, so it sorts to the top of the file list
Type "1"
Sleep 1000ms
Type@200ms "jjjjj"
Sleep 2800ms
Type "2"
Sleep 900ms

# ══ switch bookmark: the local pane re-pairs with the new bucket ═══
Type "c"
Sleep 1000ms
Type "k"
Sleep 400ms
Enter
Sleep 1300ms
Enter
Sleep 2400ms

# ══ sync, both directions ══════════════════════════════════════════
Type "s"
Sleep 3200ms
Type "d"
Sleep 2800ms
Escape
Sleep 900ms

# ══ upload one file ════════════════════════════════════════════════
Type "1"
Sleep 1000ms
Type "j"
Sleep 700ms
Type "u"
Sleep 1800ms
Enter
Sleep 2400ms
Type "2"
Sleep 2600ms

# ══ upload several at once ═════════════════════════════════════════
Type "1"
Sleep 800ms
Type "k"
Sleep 400ms
Space
Sleep 400ms
Type@220ms "jj"
Sleep 300ms
Space
Sleep 800ms
Type "u"
Sleep 1800ms
Enter
Sleep 2800ms
Type "2"
Sleep 1200ms

# ══ rename ═════════════════════════════════════════════════════════
Type@100ms "kkkk"
Sleep 400ms
Type@220ms "jj"
Sleep 700ms
Type "r"
Sleep 900ms
Backspace 24
Sleep 300ms
Type@90ms "forecast-2025q1.csv"
Sleep 700ms
Enter
Sleep 1400ms
Enter
Sleep 2600ms

# ══ copy, then paste into another prefix ═══════════════════════════
Type@100ms "kkkk"
Sleep 500ms
Type "y"
Sleep 1900ms
Type "h"
Sleep 1500ms
Type "P"
Sleep 1700ms
Enter
Sleep 2800ms

# ══ cut the copy, and move it into a dated prefix ══════════════════
Type@100ms "kkkk"
Sleep 400ms
Type@220ms "jjj"
Sleep 700ms
Type "x"
Sleep 1600ms
Type@100ms "kkk"
Sleep 400ms
Enter
Sleep 1400ms
Type "P"
Sleep 1700ms
Enter
Sleep 2800ms

# ══ delete it again ════════════════════════════════════════════════
Type@100ms "kkk"
Sleep 500ms
Type "D"
Sleep 2400ms
Type "y"
Sleep 2600ms

# ══ the events log ═════════════════════════════════════════════════
Type "E"
Sleep 4500ms
Escape
Sleep 900ms

# ══ the help screen — scrolled, so the whole thing is readable ═════
Type "?"
Sleep 4500ms
Down@400ms 7
Sleep 3500ms
Down@400ms 7
Sleep 3500ms
Type "?"
Sleep 900ms

# ══ dark theme, quit ═══════════════════════════════════════════════
Type "t"
Sleep 1800ms
Type "q"
Sleep 900ms
