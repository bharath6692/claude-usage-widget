# Claude Usage Widget

A small Windows taskbar widget that shows your Claude Code rate-limit usage
(the same `5h` / `7d` windows `/usage` and the statusline show) without
needing a Claude Code session open.

```
5h ████░░░░░░  43% · 11m
7d █████░░░░░  47% · 1d18h
```

It polls `https://api.anthropic.com/api/oauth/usage` directly using your
existing Claude Code login — no separate sign-in, no server of ours in the
middle, and it never writes to or refreshes your credentials (see
[`docs/DESIGN.md`](docs/DESIGN.md) for why).

## Install (teammates — no build required)

1. Download the latest `claude-usage-widget-<version>.zip` from
   [Releases](https://github.com/bharath6692/claude-usage-widget/releases/latest)
   and unzip it anywhere (e.g. `C:\Tools\ClaudeUsageWidget\`).
2. Run `claude-usage-widget.exe`.
3. **Windows will likely show a SmartScreen warning** ("Windows protected your
   PC") because the exe isn't code-signed. This is expected for an internal
   tool distributed as a zip rather than through an app store. Click **More
   info → Run anyway**. (A signed build removes this entirely if IT issues an
   Authenticode certificate later — see `scripts/package.ps1`.)
4. You should see:
   - Two usage bars pinned above the taskbar, near the clock, **or**
   - Just a small tray icon, if you switch to tray-only mode from the menu

Right-click the tray icon (or left-click the docked bars) for the full menu:
refresh now, update frequency, docked/tray-only display, alert thresholds,
start-with-Windows, and a detail popup showing every window (including
per-model ones on plans that have them), exact reset times, and the last
successful poll.

Settings live at `%APPDATA%\ClaudeUsageWidget\settings.json`; the log is at
`%APPDATA%\ClaudeUsageWidget\widget.log`. Both are reachable from the tray
menu ("Open settings file" / "Open log file").

## What it needs to work

- You've run `claude` and completed login at least once (so
  `~/.claude/.credentials.json` exists).
- Claude Code is periodically used, so the widget's own idle-time polling
  doesn't need to refresh anything — usage only changes when Claude Code
  runs, and Claude Code keeps that credential file current on its own.

If your sign-in has actually expired, the widget tells you exactly which of
three distinct problems it is and what to do about it (open Claude Code /
`claude /login` / not signed in at all) — it never just goes silent.

## Build from source

Requires the Rust GNU toolchain (no MSVC/Visual Studio, no admin rights):

```powershell
rustup target add x86_64-pc-windows-gnu
rustup toolchain install stable-x86_64-pc-windows-gnu
winget install BrechtSanders.WinLibs.POSIX.MSVCRT --scope user
```

**Always build through `scripts/cargo.ps1`**, not plain `cargo` — it resolves
a toolchain quirk (see `docs/DESIGN.md` "Toolchain notes") that a bare
`cargo build` will hit on a clean checkout:

```powershell
.\scripts\cargo.ps1 build            # debug
.\scripts\cargo.ps1 build --release  # release (this is what gets shipped)
.\scripts\cargo.ps1 test             # 105 tests, all headless — no window needed
```

To produce a distributable zip + update manifest in one step:

```powershell
.\scripts\package.ps1
# .\scripts\package.ps1 -SkipTests            # faster iteration
# .\scripts\package.ps1 -Notes "release notes for version.json"
```

This writes `dist/claude-usage-widget-<version>.zip` and `dist/version.json`.
To publish, fill in `download_url` in `version.json` and attach both files
to a GitHub release:

```powershell
gh release create v<version> dist/claude-usage-widget-<version>.zip dist/version.json --notes "..."
```

"Check for updates..." in the menu compares versions against the manifest
configured as `update_manifest_url` in `settings.json`. To follow GitHub
releases, set it to:

```
https://github.com/bharath6692/claude-usage-widget/releases/latest/download/version.json
```

This is notify-only: it never downloads or installs anything automatically.

## Known limitations (v1)

- The docked overlay has square corners and a fixed dark background — no
  light-theme auto-detection or rounded corners yet.
- No draggable divider for the overlay's screen position yet (there's a
  settings field for it, just no mouse handler).
- The overlay doesn't yet hide itself when the taskbar auto-hides or a
  fullscreen app takes over the monitor.

None of these affect correctness — they're the open polish items from the
original design (`docs/DESIGN.md` has the full milestone list).
