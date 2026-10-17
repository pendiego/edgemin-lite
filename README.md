# edgemin-lite

Tiny tray app that periodically empties the working set of a browser's processes (and optional extra processes) when their memory exceeds a limit. No windows; configured by INI.

Behavior modeled on Rizonesoft Edgemin; no original code used. The icon (`assets/edgemin-lite.ico`) is a recolored, modified derivative of the Edgemin icon (Rizonesoft); the MIT license does not cover it. Replace it before redistributing.
Right-click the tray icon, then **Settings...** for a window modeled on Edgemin's (browser info, live usage/peak, options, extended processes editor). Save is enabled after the first change, writes the INI and applies it immediately. Saving rewrites the INI with the keys below and `[Safemode]`; other keys from an old `Edgemin.ini` are dropped. Safe-mode arguments are edited in the INI only. `build.rs` embeds `app.manifest` (themed controls, DPI awareness); MSVC toolchain required.


Config: `edgemin-lite.ini` next to the exe (portable), else `%APPDATA%\edgemin-lite\edgemin-lite.ini`. Created with defaults on first run. An old `Edgemin.ini` can be pasted unchanged.

`[Edgemin]`
- `BrowserPath` – browser exe (its file name is the process name trimmed)
- `BoostEnabled` – 1/0, trimming on/off
- `Boost` – interval in ms (min 100)
- `LimitEnabled` / `ReduceLimit` – trim only when the group's total working set exceeds N MB
- `StartBrowser` – launch the browser at startup
- `EnableExtendedProcs` / `ExtendedProcs` – comma-separated extra exe names

`[Safemode]` – `exe=arguments`, used by the "safe mode" menu item.

License: MIT.

## Updates

`[Update]` in the INI: `Repo=owner/name` (GitHub; default `pendiego/edgemin-lite`, empty disables the updater) and `Check=1` (daily automatic check, plus once at startup). A newer release shows a tray balloon and an **Update to vX...** menu item; **Check for updates** checks on demand. Nothing is installed without confirmation.

Publish each release with an asset named exactly `edgemin-lite.exe`. The app reads its SHA-256 from the GitHub API `digest` field (added automatically to release assets by GitHub) and refuses to install on a missing or mismatching digest. No code-signing: trust rests on HTTPS to GitHub plus that hash. Tags are compared as `vMAJOR.MINOR.PATCH` against the running build's `Cargo.toml` version, so bump the version before tagging. The old exe is kept as `edgemin-lite.exe.old` until the next start. The exe's folder must be writable.
