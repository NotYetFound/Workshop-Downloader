# Workshop Downloader 0.4.1

- Improves embedded Workshop resilience: a failed WebKit renderer is retried once with the software renderer, while Reload remains available for manual recovery.
- Makes Workshop browsing smoother by throttling page-enhancement updates, avoiding duplicate status work, and limiting queue refreshes to download state rather than filesystem scans.
- Fixes the live download counter so it reflects every queued job, not just the job currently running.
- Uses SteamCMD's staged-byte information when available, so progress is measured rather than shown as an indeterminate back-and-forth animation.

## Known limitations

- The renderer recovery improves known WebKitGTK failure cases, but cannot guarantee recovery from every GPU driver or system WebKit failure.
- Windows 10/11 runtime smoke testing and real account sign-in are release validation steps. Terminal-based tests cover the prompt and resume logic.
- DepotDownloader's Windows .NET account store is outside the app-owned session directory. Use Steam's account security controls to revoke its session.
- SteamCMD on x86-64 Linux may need 32-bit system libraries and updates its own bootstrap on first use. Download availability depends on Steam and the item's access rules.
- The RPM updates through package installation. Builds without a configured updater key link to GitHub Releases.
