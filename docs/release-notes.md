# Workshop Downloader 0.4.2

- Fixes managed SteamCMD setup on Linux by accommodating Valve's bootstrap crash-report directory behavior while retaining backend write isolation.
- Automatic download selection now skips unavailable engines, falls back after engine startup and item-specific failures, and records startup issues in Preferences until cleared.
- Adds Download by ID in Downloads, startup update checks, update controls in Preferences, and clearer first-run engine guidance.
- Adds mounted-drive shortcuts to the executable chooser and removes the existing-download scanner from Preferences.
- Removes Missing files badges from Workshop pages while retaining missing-file state in Downloads.

## Known limitations

- The renderer recovery improves known WebKitGTK failure cases, but cannot guarantee recovery from every GPU driver or system WebKit failure.
- Windows 10/11 runtime smoke testing and real account sign-in are release validation steps. Terminal-based tests cover the prompt and resume logic.
- DepotDownloader's Windows .NET account store is outside the app-owned session directory. Use Steam's account security controls to revoke its session.
- SteamCMD on x86-64 Linux may need 32-bit system libraries and updates its own bootstrap on first use. Download availability depends on Steam and the item's access rules.
- The RPM updates through package installation. Builds without a configured updater key link to GitHub Releases.
