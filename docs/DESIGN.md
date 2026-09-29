# Design

## The endpoint

`GET https://api.anthropic.com/api/oauth/usage`, undocumented but stable in
practice. Required headers:

```
Authorization: Bearer <accessToken>
anthropic-beta: oauth-2025-04-20
User-Agent: claude-code/<version>
Content-Type: application/json
```

Without the `claude-code/*` User-Agent prefix, the endpoint drops you into an
aggressively throttled bucket and you get persistent 429s almost
immediately. `usage/client.rs::detect_claude_code_version` reads
`~/.claude/.last-update-result.json` so the User-Agent tracks whatever
version of Claude Code is actually installed, rather than a hardcoded string
that silently goes stale.

Response shape (fields not shown are ignored — see below):

```json
{
  "limits": [
    {"kind": "session", "percent": 15, "severity": "normal", "resets_at": "2026-07-29T12:00:00.331570+00:00", "is_active": false},
    {"kind": "weekly_all", "percent": 68, "severity": "normal", "resets_at": "2026-07-29T09:00:00.331594+00:00", "is_active": true}
  ],
  "five_hour": {"utilization": 15.0, "resets_at": "..."},
  "seven_day": {"utilization": 68.0, "resets_at": "..."},
  "seven_day_opus": null, "seven_day_sonnet": null, "seven_day_cowork": null,
  "extra_usage": {"is_enabled": false, "monthly_limit": null, "used_credits": null, ...}
}
```

**`limits[]` is what gets rendered, not the typed `five_hour`/`seven_day`
fields.** It's the normalized view with server-computed `severity` and
`is_active`, and it grows to cover `seven_day_opus`/`_sonnet`/`_cowork` — null
on a Team plan, populated on plans that have per-model windows. The typed
fields are kept only as a fallback for a response that omits `limits[]`
entirely (`usage/model.rs::RawUsage::into_snapshot`).

`resets_at` is an ISO-8601 string here, not the Unix-epoch-milliseconds the
Claude Code statusline payload uses — don't reuse statusline parsing logic
against this endpoint.

Parsing is `#[serde(default)]` throughout with unknown fields ignored: this
is an undocumented endpoint and *will* change shape without notice. A
response that parses as valid JSON but yields zero recognizable windows logs
the raw body (never the token) so a shape change can be diagnosed from a
teammate's log rather than guessed at (`usage/client.rs::classify`).

## Why credentials are read-only

`~/.claude/.credentials.json` holds `accessToken`, `refreshToken`,
`expiresAt` (epoch ms), `refreshTokenExpiresAt`, `scopes`,
`subscriptionType`, `rateLimitTier`. The widget only ever reads this file
(`creds.rs::load`) — it never refreshes, writes, or logs the token itself
(`Credentials::token` returns a `Zeroizing<String>`, and `Debug` is
hand-implemented to redact it).

Refreshing the OAuth token ourselves would rotate the refresh token and race
Claude Code's own writes to the same file — losing that race breaks a
teammate's login entirely. It's also unnecessary: **usage only moves when
Claude Code runs, and Claude Code refreshes the credential file whenever it
runs.** So the widget re-reads the file on every poll and simply skips the
network request once `expiresAt` has passed, surfacing that as the
self-healing "access token expired" state rather than an error.

The one thing that *does* change while the machine sits idle is a window
resetting on its own schedule — and the response already tells us
`resets_at`, so once that passes, the widget zeroes that row locally and
marks it provisional until the next successful poll confirms it
(`usage/model.rs::project`). Correct display, zero write access to anyone's
credentials.

### The three distinct auth problems

| State | Message | Why it's distinct |
|---|---|---|
| Not signed in | *Not signed in to claude.ai — run `claude` and complete login* | No credentials file, or missing the `claudeAiOauth` key. Onboarding, not a regression — fires immediately, no debounce. |
| Access token expired | *Claude sign-in expired — open Claude Code to refresh it* | `expiresAt` passed but `refreshTokenExpiresAt` hasn't. Self-healing the moment Claude Code runs again. |
| Refresh required | *Claude sign-in expired — run `claude /login` to sign in again* | `refreshTokenExpiresAt` passed, or the server returned 401/403 despite a locally-valid token (revoked server-side). Genuinely needs re-login. |

Collapsing these into one generic "not authenticated" message would tell
someone to run `claude /login` when opening Claude Code was all they needed
— exactly the kind of wrong advice that gets a shared tool uninstalled
(`creds.rs::AuthProblem`).

## Architecture

Single binary, `#![windows_subsystem = "windows"]`, no async runtime. One
background poller thread publishes into `Arc<Mutex<SharedState>>` and
`PostMessage`s the UI thread; everything else is synchronous.

```
src/
  main.rs        entry point, --print-once diagnostic path, DPI awareness, wiring
  config.rs      settings.json load/save/sanitize
  creds.rs       .credentials.json parsing, AuthProblem classification
  update.rs      manual "check for updates": fetch version.json, compare semver
  usage/
    model.rs     RawUsage -> UsageSnapshot, local reset-zeroing, staleness
    client.rs    HTTPS GET, 429/Retry-After backoff
    poller.rs    background thread, SharedState, PollOutcome
    alerts.rs    threshold-crossing + auth-transition toast bookkeeping
  ui/
    theme.rs     color ramp (shared with statusline-command.ps1's thresholds)
    layout.rs    pure geometry: rows -> cell rects at a given DPI scale
    render.rs    GDI paint of a layout into a device context
    taskbar.rs   SHAppBarMessage taskbar-rect/edge/autohide query
    overlay.rs   the docked window: creation, positioning, WM_PAINT, click-through
    popup.rs     detail panel: every window, extra_usage, last poll, dismiss-on-blur
    icon.rs      dynamic tray icon rendering (ring/warning glyph as a DIB)
    menu.rs      pure menu model + command handling (testable without Win32)
    tray.rs      the Win32 shell: hidden window, Shell_NotifyIcon, wires everything above
```

The data layer (`creds`, `usage/*`) is fully headless and unit-tested without
touching a HWND. `ui/layout.rs` is pure geometry, also unit-tested without a
window. `ui/render.rs`, `overlay.rs`, `popup.rs`, `tray.rs` are the only
places that call into GDI/Win32 directly.

### Display modes

`Settings::display_mode` is `Docked` (bars painted above the taskbar) or
`TrayOnly` (icon only, detail popup on click). `TrayApp::sync_overlay`
creates/destroys the overlay window to match on every settings change and
every successful poll.

### Alerts

`usage/alerts.rs::AlertState` is pure and independently tested: a threshold
fires once per window *instance* (keyed on `resets_at`, so a new window
re-arms it), and an auth problem fires once per *transition* into that state,
re-arming after 4 hours if unresolved. `TrayApp::process_alerts` feeds it
live poll outcomes and turns what it returns into `Shell_NotifyIconW`
balloons (`NIF_INFO`), gated on the "Alerts enabled" setting.

## Toolchain notes (read this before touching the build)

This project targets `x86_64-pc-windows-gnu` specifically so it needs no
MSVC/Visual Studio/Windows SDK install — everything lives under
`%USERPROFILE%\.rustup` and one user-scope `winget install`, no admin rights.
That choice is not free: the GNU toolchain on Windows is far less exercised
than MSVC for `windows-rs`, and this repo hit two real toolchain bugs getting
there. **Always build through `scripts/cargo.ps1`**, never bare `cargo` —
both fixes below are wired through it and `.cargo/config.toml`.

**1. No CRT sysroot in the installed mingw-w64 package.** The WinLibs
package (`BrechtSanders.WinLibs.POSIX.MSVCRT`, installed for its `as.exe`
assembler — rustup's own bundled toolchain doesn't ship one) does not include
`crt2.o` / `libkernel32.a` / friends under its own tree, so linking fails
with `ld: cannot find crt2.o` and a long list of missing `-l*` libraries.
rustup's toolchain *does* bundle exactly these files (under
`lib/rustlib/x86_64-pc-windows-gnu/lib/self-contained/`) — they're just not
used by default. Fix: `-C link-self-contained=yes` in
`.cargo/config.toml`, which tells rustc to link against its own bundled
copies instead of searching the external mingw install.

**2. rustup's bundled `dlltool.exe` can't find an assembler on its own.**
Raw-dylib crates (`windows-sys`, `windows-result`, `windows-strings`, ...)
need `dlltool` to build import libraries, and it fails with a bare
`dlltool.exe: CreateProcess` — it's trying to spawn an assembler by some
name that never resolves via PATH search in this environment. `dlltool` does
accept an explicit `-S <path>` flag naming the assembler to use, and passing
that by hand works every time. **Do not "fix" this by copying `as.exe`
somewhere else** — it depends on sibling DLLs that live alongside it in its
real install directory; a bare copy fails to even load
(`STATUS_DLL_NOT_FOUND`, which `dlltool` reports as the cryptic "exited with
status 53" — `0xC0000135 & 0xFF == 53`). The actual fix is
`scripts/dlltool-wrapper.bat`, a tiny shim that forwards every argument to
the real `dlltool.exe` plus `-S <as.exe>` pointed at the assembler's real,
untouched install location. `.cargo/config.toml` points rustc at the wrapper
via `-C dlltool=<absolute path>` — it must be absolute, since rustc invokes
this once per crate from that crate's own source directory (including
dependencies under `~/.cargo/registry`), not from the workspace root.

Both issues silently *don't* reproduce against a `target/` directory that
already has cached build artifacts from a build done under different
environment conditions — that's exactly how they went unnoticed through
several milestones of this project before surfacing on the first from-clean
release build. If you ever suspect a "works on my machine" toolchain issue
here, `Remove-Item -Recurse -Force target` and rebuild before concluding
anything.

Switching to the MSVC toolchain later (if IT provisions a Visual
Studio Build Tools install) is a one-line `rustup` default-host change and
would remove all three of the above at the cost of needing admin rights and
a much larger toolchain footprint.

## Known risks

1. **`oauth/usage` is undocumented** and can change or disappear without
   notice. Mitigated by tolerant parsing and logging the raw body on a parse
   failure. If it goes away entirely, the fallback (not built) would be
   reading rate-limit headers off a normal Messages API response.
2. **Unsigned exe** triggers a SmartScreen warning on first run for every
   teammate (documented in the README). A Teleflora IT Authenticode
   certificate would remove this without any code change —
   `scripts/package.ps1` has an obvious slot to add a signing step.
3. **Win11 taskbar internals are undocumented** and shift between Windows
   builds. Mitigated by anchoring to the taskbar's right edge via the public
   `SHAppBarMessage` API rather than locating undocumented XAML child
   windows, and by shipping tray-only mode as an always-working fallback.
