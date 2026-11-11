# Milestone 4 release validation

Workshop Downloader keeps its existing application identifier, SQLite store, and desktop layout. Debug is under **Preferences → Application and diagnostics**.

## Build and release

- GitHub CI runs formatting, Clippy, Rust tests, the frontend build, and a Tauri build on native Windows and Linux runners.
- Pushing a tag matching the application version builds an x86-64 NSIS installer, AppImage, and RPM. Linux release CI uses Ubuntu 22.04 as its baseline. Local Nobara builds are used for Nobara smoke testing. Test the CI AppImage separately on its Ubuntu baseline and on a second supported distribution.
- The AppImage includes GStreamer base/good media plugins. On Fedora-family local build hosts, install patchelf and set `GSTREAMER_PLUGINS_DIR` to the selected 64-bit plugin directory and `GSTREAMER_HELPERS_DIR` to `/usr/libexec/gstreamer-1.0`. The RPM declares the GStreamer runtime packages.
- Successful builds upload workflow artifacts, then the draft release job collects and checksums the three packages. It requires an existing tag and refuses to modify a published release.
- Configure the repository variable `WORKSHOP_UPDATER_PUBLIC_KEY` with the Tauri public key and the Actions secret `TAURI_SIGNING_PRIVATE_KEY` with its matching private key. If applicable, set `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`. For the unsigned path, leave both the key variable and signing secret unset. The draft job verifies each updater artifact with Minisign and checks the signed version before writing `latest.json`. RPM updates are installed through the package manager.
- Tauri updater signatures verify app updates. Windows Authenticode signing and publisher reputation are separate distribution steps. The draft workflow does not publish automatically.

[Tauri GitHub packaging](https://v2.tauri.app/distribute/pipelines/github/) · [Tauri updater](https://v2.tauri.app/plugin/updater/) · [Tauri AppImage](https://v2.tauri.app/distribute/appimage/)

## Backends and account sessions

Engine setup resolves the official source and platform asset before asking the user to install. It validates the archive, verifies the published SHA-256 digest if one is supplied, probes the executable, and only then switches the managed selection. The previous managed version stays available for rollback. Manual executable paths take precedence and are outside managed updates. SteamCMD's official bootstrap is distributed over HTTPS without an archive checksum on the setup page. DepotDownloader's selected release supplies a size and may also supply a GitHub asset digest. Neither binary is redistributed with the application. DepotDownloader's distribution includes its GPL-2.0 license. SteamCMD's runtime requirements and distribution terms are handled by Valve's official setup.

The engines use documented CLI arguments for anonymous, username, and DepotDownloader QR sign-in. Passwords and Guard codes go to the backend's private terminal. Authentication output and QR challenges are excluded from Debug transcripts. The Linux DepotDownloader runtime uses a private home and the CLI's remembered session option. SteamCMD uses its own runtime and home. Changing the username selects a different account. Clear session removes the appropriate app-owned runtime while the worker is idle. Windows DepotDownloader resolves its account store through .NET and Windows known folders. The app does not request a persistent Windows DepotDownloader token or delete that shared store. Steam's account security controls can revoke the session.

[Valve SteamCMD setup](https://developer.valvesoftware.com/wiki/SteamCMD) · [Official DepotDownloader CLI and license](https://github.com/SteamRE/DepotDownloader) · [DepotDownloader account storage](https://github.com/SteamRE/DepotDownloader/blob/DepotDownloader_3.4.0/DepotDownloader/AccountSettingsStore.cs)

## Additional source evaluation

| Candidate | Finding |
| --- | --- |
| [WorkshopDL](https://github.com/fidget77/WorkshopDL) | Windows-oriented GUI that delegates to SteamCMD. It offers no independently documented cross-platform download API or new item access. Authentication and item support remain bounded by SteamCMD. The repository reports an MIT license. The Windows GUI runtime is an additional distribution dependency. |
| [davidroman0O/steam-workshop-downloader](https://github.com/davidroman0O/steam-workshop-downloader) | Documented CLI wrapper for SteamCMD. It inherits SteamCMD's installation, login, ownership, and Workshop support. It offers no independent download path and the repository has no detected license. |
| [Steamworks ISteamUGC](https://partner.steamgames.com/doc/api/ISteamUGC) | Documented native SDK API for a Steamworks-enabled application, with game/application context and Steam client dependencies. It is not a standalone general-purpose Workshop CLI and has SDK distribution terms. |

SteamCMD and DepotDownloader remain the engines. No additional candidate offers a validated, independent way to improve downloads within the existing desktop app's scope.

## Manual smoke checklist

Run Windows steps on actual Windows 10 and 11 systems. A compilation result does not verify WebView2, ConPTY, the installer, or desktop behavior.

1. Install the NSIS build as a standard user, including on a machine without WebView2. Verify the Start menu, desktop icon where installed, launch, uninstall, and update over an existing install. Check that settings and history survive the update.
2. On Nobara KDE Wayland, launch the AppImage from a file manager with a normal GUI environment. Install the RPM on Fedora or Nobara and launch it from the app menu. Exercise resizing and mixed-DPI scaling, browser history, search, collections, GIF/video previews, and folder selection. Repeat the AppImage launch on the Ubuntu baseline and a second supported desktop.
3. In a fresh profile, confirm the default destination, install one official engine, and open Workshop. Check detection with a reduced PATH, a manual executable, a missing tool, and an incompatible executable. Check the version, update, and rollback of a managed engine while idle. Attempt a managed update while a job is running.
4. Download a public item with each engine and with Automatic. Then download a collection and a bulk selection. Verify bytes, history, progress, badges, duplicate handling, per-game roots, Unicode titles, and Steam libraries on separate volumes.
5. Test password, Guard, mobile approval, and DepotDownloader QR with an authorized test account. Verify prompt transitions, account switching, remembered login on supported platforms, session expiration and clearing, and a denied item. Check that the Terminal and export contain no passwords, tokens, codes, QR data, or exported paths and account identifiers.
6. Exercise a full or read-only destination, a Windows locked file, interrupted network, cancellation, exit during transfer, missing/deleted item, and a Steam metadata rate limit. Reopen the app and retry. Confirm failed jobs never publish partial output and that queue/history survive restart.
7. Test a draft signed update from an older signed build with queued downloads. Verify interruption, retry, rejection of a corrupt payload, and retention of settings and queue records after a successful restart.

## Local verification scope

The Linux packages in `src-tauri/target/release/bundle` are local Nobara builds. The AppImage launched through FUSE on KDE Wayland and started its bundled backend helper. The RPM file list and dependencies were inspected. The frontend build, formatting, Clippy, and 49 native tests pass locally. The draft workflow syntax passed actionlint. Release assembly was tested with unsigned fixtures, ephemeral Tauri signing keys, and rejection of a modified signed payload. The native tests cover archive path handling, session prompt behavior, QR parsing, library discovery, queue recovery, and the existing download and SQLite regressions. Linux DepotDownloader downloaded the public Global Positions item anonymously both directly and through the packaged AppImage helper. The resulting payloads matched. The standalone engine also emitted a real QR challenge. The QR challenge was checked without signing into an account. The standalone SteamCMD bootstrap started and connected anonymously, then timed out waiting for Steam user information. A current SteamCMD transfer, live authenticated downloads, and Windows packages and runtime still need verification.
