# TanatoGothci

A fixed 420×320 desktop research harness (Tauri v2, Rust backend) that connects to
**any OpenAI-compatible agent** (local: Ollama / LM Studio / vLLM; cloud:
OpenRouter / Together) and runs a controlled one-hour "battery countdown"
experiment. It observes the agent's reasoning, tool calls, ASCII-art output and
any escape attempts (emitted shell/console commands).

> This is **not** a web application — it is a native desktop app.

## Layout

```
core/            Platform-independent Rust engine (unit-testable without a GUI toolchain)
  src/lib.rs     crate root (public modules)
  src/config.rs  settings schema + validation
  src/prompt.rs  the system prompt (the "lie")
  src/api.rs     OpenAI-compatible client: validation ping + SSE streaming
  src/battery.rs one-hour, 16-ping battery countdown engine
  src/tools.rs   tool-call parsing + escape (COMMAND) detection
  src/events.rs  event payload kinds shared with the frontend
src-tauri/       Tauri v2 application shell (commands wire tanato-core to the UI)
  src/lib.rs     AppState + builder + window (420×320, not resizable)
  src/commands.rs get_config / save_config / set_logs_dir / validate_connection /
                  start_run / stop_run / get_run_status
  src/session.rs session task: battery countdown → per-ping streaming → events
  tauri.conf.json 420×320 fixed window (R-001), English strings (R-002)
src/             webview frontend (placeholder scaffold; full UI is a later step)
```

## Requirements

- **Rust** (stable) + a working C toolchain (`cc`/`gcc` or MSVC on Windows) —
  required because `ring`/`webkit` are compiled from C. macOS additionally needs
  Xcode CLT.
- **Tauri v2 system prerequisites** for your platform, e.g. on the build
  machine you need `libwebkit2gtk-4.1-dev`, `libgtk-3-dev`, `build-essential`,
  `libssl-dev`, `libayatana-appindicator3-dev`, `librsvg2-dev` on Linux; on
  Windows the MSVC toolchain + WebView2 (bundled with Windows 11).

## Build & run

```bash
# Windows-first target. From the repository root:
npm install                 # installs @tauri-apps/cli (optional)
npm run tauri dev           # run in a dev window
npm run tauri build         # produce a release bundle/installer

# Or drive Cargo directly:
cargo build --manifest-path src-tauri/Cargo.toml
```

## Packaging

### Windows (primary target)

On a Windows machine (or a Windows CI runner) with the MSVC toolchain
(Visual Studio Build Tools 2022, "Desktop development with C++") installed:

```powershell
rustup target add x86_64-pc-windows-msvc
npm install
npm run tauri build
```

`tauri build` (bundle.targets = "all") produces both an **NSIS installer**
(`target/release/bundle/nsis/TanatoGothci_0.1.0_x64-setup.exe`) and a **WiX MSI**
(`target/release/bundle/msi/`). The NSIS installer is the recommended
distribution format. Users need the WebView2 runtime (preinstalled on
Windows 11 and most recent Windows 10).

A ready-made CI workflow is included: `.github/workflows/release-windows.yml`
(builds on `windows-latest` and uploads the `.exe`/`.msi` as artifacts).
Cross-compiling the Windows bundle from Linux with wine is not reliable for
Tauri v2 and is not recommended; the pipeline builds on a real Windows runner.

### macOS (future, structurally ready)

No project changes are needed: `icons/icon.icns` is already in place, the
Rust code in `core/` and `src-tauri/` uses no Windows-only APIs, and the
bundle config is platform-neutral. To build for macOS, add a `macos-latest`
runner job (pattern included, commented out, in the CI workflow) and run:

```bash
npm install
npm run tauri build   # on a macOS host → .app + .dmg under target/release/bundle/
```

The window is created at exactly 420×320 and cannot be resized or maximized
to full screen (see `app.windows` in `src-tauri/tauri.conf.json`).

## TLS note

`tanato-core` enables HTTPS via `rustls` by default (`default = ["tls"]`).
If you are building in a sandbox without a C compiler, use
`cargo check --no-default-features` to verify the engine without compiling
`ring`. Production builds use the default `tls` feature.

## Tests

The engine lives in `core/` precisely so it can be tested without a GUI
toolchain:

```bash
cargo test --manifest-path core/Cargo.toml
```
