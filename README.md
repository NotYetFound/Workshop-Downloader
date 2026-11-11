# Workshop Downloader

Workshop Downloader browses Steam Workshop in a native desktop webview and downloads individual items, selections, and collections through SteamCMD or DepotDownloader.

## Build and run

Use stable Rust and Node.js 22.12 or later in a supported release line. On Nobara and Fedora, install the [Tauri system prerequisites](https://v2.tauri.app/start/prerequisites/#linux). Linux downloads require [Landlock](https://docs.kernel.org/userspace-api/landlock.html) with ABI 3 or later enabled. On Windows, install [Visual Studio Build Tools with Desktop development with C++ and WebView2](https://v2.tauri.app/start/prerequisites/#windows). Interactive downloaders on Windows use ConPTY, available in Windows 10 version 1809 and later.

```sh
npm ci
npm run tauri dev
```

```sh
npm run check
cargo test --locked --manifest-path src-tauri/Cargo.toml
npm run tauri build -- --no-bundle
```

Both dependency sets have lockfiles. Rust tests regenerate the TypeScript contracts in `src/lib/contracts`.

## Set up a downloader

Workshop Downloader detects executables on the system PATH. On Linux it also looks for a SteamCMD installation under `~/.local/share/steamcmd`. Open **Preferences → Download engines** to select an executable, run the operational checks, or open the official setup pages. Save preferences after changing paths or the default backend.

- For [Valve SteamCMD](https://developer.valvesoftware.com/wiki/SteamCMD), extract the official distribution to its own directory. Select `steamcmd.sh` next to its Linux binaries or `steamcmd.exe` on Windows. On Linux, use the actual launcher from the distribution when a package wrapper cannot be isolated. The app copies the required distribution files into its own runtime and uses a separate home and install directory.
- For [SteamRE DepotDownloader](https://github.com/SteamRE/DepotDownloader/releases/latest), extract the official release for your OS and select the standalone executable. The app uses its documented published-file, directory, and verification options. Each job runs in its own working directory.
- **Automatic** prefers an eligible SteamCMD installation, then DepotDownloader. An unavailable executable or unsupported operation can switch to the next eligible provider. Account, ownership, item, and network failures require an explicit retry.

Both tools are installed separately. The app opens their official sources and does not redistribute their binaries. DepotDownloader is [GPL-2.0 licensed](https://github.com/SteamRE/DepotDownloader/blob/master/LICENSE).

## Download and manage items

1. Open an individual item in **Browse** and left-click **Download…** in the app toolbar to queue it immediately using your saved default backend and sign-in preference.
2. Right-click the same button for a native overlay menu. Toggle **Sign in with Steam** for that menu invocation if needed, then click an engine to queue one download immediately. The next left-click uses your saved defaults. Save your default backend, sign-in preference, and Steam account name in **Preferences → Download engines**. The sidebar shows the current mod, its Workshop size, its position in the batch, and the progress reported by the engine. Standalone downloads display **1/1**. The bar shows activity when a percentage is unavailable. Failures appear in a separate dismissible alert. Press Enter in the address bar to navigate; **Copy** copies the current page URL.
3. Use **Downloads** to cancel an active or queued job, retry, open the folder, expand the log, or remove a terminal job from history. Removing history keeps installed files and their Downloaded badge.
4. **Download again…** creates another copy. Explicit replacement remains available when retrying a replacement job; batches never replace existing folders.

The login flow passes the account name as the backend’s documented argument. The trusted shell collects the password or Steam Guard code in Preferences only when the backend prompts and sends it to the backend’s private terminal. Mobile approval is shown when the backend requests it. Workshop Downloader does not write passwords or codes to SQLite, arguments, or its diagnostic logs. SteamCMD uses its own managed Steam home for any session data. An account-backed job still queued after restart waits for the account name through the resume action.

## Collections, selections, and batches

Open a collection’s normal Workshop URL and choose **Download Collection…**. The trusted app preview lists the collection name, game, all unique leaf items, current downloaded states, and unavailable members. Select All / Deselect All act only on eligible items. Downloaded and externally detected items are excluded by default; **Include downloaded items** explicitly permits selecting them again. A native menu engine action on a collection opens this required preview with that engine chosen for this operation.

On supported Steam browse/search cards, use the selection checkbox or compact download arrow. **Download Selected** appears only while items are selected. Multiple selections open a preview with **Download Selected**. Single items show a compact **Download** action in the app while the Steam page stays visible. Back dismisses either view and keeps the current Workshop page. They do not silently execute native commands. Steam search, sorting, filtering, subscriptions, and navigation continue normally.

**Downloads** groups jobs into expandable batches with completed, failed, skipped, and cancelled counts. Cancel remaining jobs or retry failed/cancelled jobs and resume account jobs using saved preferences. Each job retains its own log, retry action, and destination-folder action. Batch members’ histories are retained with the batch rather than individually removable. Progress counts completed items and never estimates aggregate byte percentages.

Collection metadata uses [Valve’s GetCollectionDetails and GetPublishedFileDetails endpoints](https://partner.steamgames.com/doc/webapi/ISteamRemoteStorage). The public metadata endpoint can omit file type, so the collection endpoint supplies the authoritative collection check. Nested traversal removes duplicates and detects repeated/cyclic collections. Limits are 2,000 unique items, 128 collections, and 16 nesting levels. Requests run serially in chunks of at most 50 IDs, with a minimum delay and bounded response size. Private/deleted items appear as unavailable; cross-game members cannot enter a collection batch. Queue admission rechecks public metadata and commits jobs and membership together.

## Recognize existing content

In **Preferences → Existing downloads**, choose a directory and scan. Steam/SteamCMD discovery reads `appworkshop_<appid>.acf` and its installed entries, then verifies the corresponding `content/<appid>/<publishedfileid>` tree. Choose the Workshop directory containing the ACF files, rather than a bare numeric mod folder. Workshop Downloader inventory files are also recognized when present in copied exports.

For DepotDownloader output containing `.DepotDownloader/<appid>_<manifest>.manifest`, supply the matching Workshop item URL. Valve’s published-file metadata must match both the game and content manifest ID. A manifest alone cannot reliably identify a published-file ID, so the app does not infer one from the folder name. Known manifest byte sizes must match the captured payload. Unidentifiable, unsafe, missing, or incomplete content produces scan notes.

External records retain their source, path, and file inventory separately from app-managed installations. Scanning is read-only: it never adds markers to Steam folders, moves files, or allows external paths to become replacement targets. Repeated scans update the same identity/location record. Size and file inventory verification detects missing files and changed sizes; it does not claim cryptographic verification of every external file. Use a rescan after intentionally modifying external content.

## Destinations and recovery

Preferences default to the operating system’s Downloads folder under `SteamWorkshop`. Game organization uses the saved game’s name, with its App ID added if two normalized game names collide. An unknown game uses a folder such as `App 322330`. A per-game override is used directly as the root. The default mod folder name is the Workshop title. You can also use title plus ID or ID alone. When Valve omits a title, the published-file ID is used.

The worker runs one job at a time in FIFO order among jobs ready to run. On Linux, Landlock restricts downloader writes to the job staging area and the provider’s private runtime. DepotDownloader also gets the narrow system .NET named-mutex directory required by its runtime. On Windows, CLI providers run with the user’s filesystem permissions and a Job Object for process-tree cancellation. Finalization validates and publishes files on both platforms. It requires a backend completion marker, a successful exit, and a nonempty tree matching the byte total when the CLI reports one. The staged files are copied to a private sibling directory, verified, and renamed into place without overwriting an occupied destination. Unsafe names, links, reparse points, and case-insensitive filename conflicts are rejected. A small inventory file inside each completed folder lets the app detect missing or changed-size files after restart.

SQLite schema version 3 stores the queue, histories, bounded logs, current installations, collection metadata/membership, previews, batches and their job relationships, and separate external inventories. Migrations preserve version 1/2 settings and job records. Account passwords and Guard codes are never persisted. Current content states distinguish queued, downloading, downloaded, external, missing, failed, and not downloaded; file inventories are cached for five seconds, invalidated on imports and completion, and rebuilt after restart. An interrupted transfer can be retried. If publication finished before the app exited, recovery can verify the folder and commit its record. A previous folder is restored when a replacement was interrupted before publication. An occupied destination is left intact, and the old backup is kept if it cannot be restored safely.

## Browser and security

The Svelte shell and the native Steam child webview have separate labels and profiles. Windows uses the Tauri WebView2 child. Linux uses a GTK overlay to position the child inside the main window despite [Wayland child-view sizing issues](https://github.com/tauri-apps/tauri/issues/15656). The child is hidden on Downloads, Preferences, and trusted previews while its history is retained. Right-click engine actions use Tauri’s native context menu, which overlays the child without resizing it.

Steam gets no Tauri capability. The Steam page has no always-visible Workshop Downloader toolbar. Compact card controls and an on-demand selection action can request a local confirmation or preview. The trusted shell makes the actual queue request, and Rust resolves the consumer App ID, title, and update time through [Valve’s GetPublishedFileDetails endpoint](https://partner.steamgames.com/doc/webapi/ISteamRemoteStorage#GetPublishedFileDetails). Only Workshop IDs and status labels are sent back to the Steam page. No paths, account information, or logs cross into remote content. Browse-card badges appear beside the rating stars, above the item title; card identification uses canonical Workshop links rather than Steam’s hashed layout classes. Filesystem paths, process execution, and authentication remain in the local shell and Rust backend.

| Module | Responsibility |
| --- | --- |
| `src-tauri/src/downloads/manager.rs` | Queue scheduling, cancellation, retry, authentication state, and recovery |
| `src-tauri/src/downloads/providers.rs` | Provider discovery, selection, structured CLI arguments, and progress/error parsing |
| `src-tauri/src/downloads/process.rs` | Private terminals, process-tree cleanup, and transient prompt input |
| `src-tauri/src/downloads/files.rs` | File verification, atomic publication, replacement, and inventories |
| `src-tauri/src/storage.rs` | Transactional SQLite migrations, batch membership, and records |
| `src-tauri/src/steam.rs` | Authoritative metadata, bounded collection traversal, and URL validation |
| `src-tauri/src/workshop/commands.rs` | Trusted previews, batch admission, and external scan orchestration |
| `src-tauri/src/workshop/external.rs` | Read-only manifest discovery and external file inventories |
| `src-tauri/src/injection/workshop.js` | Workshop item actions and dynamic badges |

## Desktop smoke test

On Linux, open [Global Positions](https://steamcommunity.com/sharedfiles/filedetails/?id=378160973), queue it anonymously with an available provider, then inspect its files and the Downloads log. Verify the badge on the item and in search, and reopen the app to confirm the record persists. Use a temporary download root when testing replacement or removing files.

The development smoke tests ran both official providers through the embedded app on Nobara KDE Wayland. Milestone 3 resolved the public 21-item DST mod list collection, downloaded Global Positions and Health Info as one selected batch through DepotDownloader, verified the Global Positions payload against its reference, exercised browse selection and trusted previews, and recognized existing DepotDownloader content without duplicating its record on rescan. Restart checks preserved batch history and external records, detected a missing payload, and verified external fallback status on the item toolbar and browse cards. Unit tests cover parsing, cycles, unavailable and cross-game items, batch rollback/cancellation/retry, external manifests, copied inventories, and database migrations. The Windows GNU target passed compile checking. Authenticated terminal input and cancellation are covered by mocked-process tests. Live credentialed Steam sign-in and the Windows desktop runtime require hands-on verification. Milestone 3 adds collection and bulk previews, persistent batches, and read-only external discovery. Future work can add richer provider progress and packaging/signing; there is no mod installation, load order, subscription management, or background updating.
