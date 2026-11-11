# Workshop Downloader 0.4.0

- First-run setup confirms the download directory and helps set up SteamCMD or DepotDownloader.
- Preferences shows detected engine versions, offers official managed installs and rollback, and includes Debug and a redacted diagnostics export.
- Jobs can hold for Steam sign-in and resume on the same engine. DepotDownloader can display its Steam mobile QR challenge in the sign-in window.
- Steam library scans can follow the Workshop directories listed in the selected Steam installation's library manifest.
- Linux builds produce an AppImage and RPM. Windows builds target a per-user NSIS installer. Signed releases can offer in-app updates on Windows and AppImage.

## Known limitations

- Windows 10/11 runtime smoke testing and real account sign-in are release validation steps. Terminal-based tests cover the prompt and resume logic.
- DepotDownloader's Windows .NET account store is outside the app-owned session directory. Use Steam's account security controls to revoke its session.
- SteamCMD on x86-64 Linux may need 32-bit system libraries and updates its own bootstrap on first use. Download availability depends on Steam and the item's access rules.
- The RPM updates through package installation. Builds without a configured updater key link to GitHub Releases.
