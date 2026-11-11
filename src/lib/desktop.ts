import type { BackendRelease } from './contracts/BackendRelease';
import type { ApplicationInfo } from './contracts/ApplicationInfo';
import type { UpdateInfo } from './contracts/UpdateInfo';
import type { TerminalSession } from './contracts/TerminalSession';
import type { DownloadActivity } from './contracts/DownloadActivity';
import { invoke } from '@tauri-apps/api/core';
import { confirm, open, save } from '@tauri-apps/plugin-dialog';
import type { AuthPrompt } from './contracts/AuthPrompt';
import type { BackendChoice } from './contracts/BackendChoice';
import type { BackendInfo } from './contracts/BackendInfo';
import type { DownloadLog } from './contracts/DownloadLog';
import type { ItemDownloadState } from './contracts/ItemDownloadState';
import type { QueueOptions } from './contracts/QueueOptions';
import type { BrowserAction } from './contracts/BrowserAction';
import type { BrowserBounds } from './contracts/BrowserBounds';
import type { BrowserState } from './contracts/BrowserState';
import type { Destination } from './contracts/Destination';
import type { DownloadJob } from './contracts/DownloadJob';
import type { ItemInspection } from './contracts/ItemInspection';
import type { Settings } from './contracts/Settings';

import type { WorkshopPreview } from './contracts/WorkshopPreview';
import type { DownloadBatch } from './contracts/DownloadBatch';
import type { BatchProgress } from './contracts/BatchProgress';
import type { ContentStatus } from './contracts/ContentStatus';
import type { ExternalSummary } from './contracts/ExternalSummary';
import type { ScanReport } from './contracts/ScanReport';

export const contentLabels: Record<ContentStatus, string> = {
  notDownloaded: 'Not downloaded', queued: 'Queued', downloading: 'Downloading', downloaded: 'Downloaded',
  downloadedExternally: 'Downloaded externally', missingFiles: 'Missing files', failed: 'Failed',
};

export const desktop = {
  isCollection: (id: string) => invoke<boolean>('workshop_is_collection', { id }),
  previewWorkshop: (collectionId: string | null, ids: string[] = []) => invoke<WorkshopPreview>('preview_workshop', { collectionId, ids }),
  queueBatch: (previewId: string, selected: string[], options: QueueOptions, redownload: boolean) => invoke<DownloadBatch>('queue_workshop_batch', { previewId, selected, options, redownload }),
  batches: () => invoke<BatchProgress[]>('list_batches'),
  cancelBatch: (id: string) => invoke<void>('cancel_batch', { id }),
  retryBatch: (id: string, options: QueueOptions) => invoke<void>('retry_batch', { id, options }),
  contentStates: () => invoke<Record<string, ItemDownloadState>>('content_states'),
  scanExternal: (directory: string, workshopUrl: string | null) => invoke<ScanReport>('scan_external', { directory, workshopUrl }),
  openExternalFolder: (id: string, path: string) => invoke<void>('open_external_folder', { id, path }),
  externalItems: () => invoke<ExternalSummary[]>('list_external_items'),
  takeWorkshopPreview: () => invoke<string[] | null>('take_workshop_preview'),
  copyWorkshopUrl: () => invoke<void>('copy_workshop_url'),
  getSettings: () => invoke<Settings>('get_settings'),
  saveSettings: (settings: Settings) => invoke<void>('save_settings', { settings }),
  browserState: () => invoke<BrowserState>('browser_state'),
  browserAction: (action: BrowserAction) => invoke<void>('browser_action', { action }),
  navigate: (destination: Destination) => invoke<void>('navigate_workshop', { destination }),
  setBrowserSlot: (bounds: BrowserBounds | null) => invoke<void>('set_browser_slot', { bounds }),
  inspectItem: (address: string) => invoke<ItemInspection>('inspect_item', { address }),
  downloads: () => invoke<DownloadJob[]>('list_downloads'),
  queueDownload: (address: string, options: QueueOptions, previewId?: string) => invoke<DownloadJob>('queue_download', { address, options, previewId }),
  retryDownload: (id: string, options: QueueOptions) => invoke<void>('retry_download', { id, options }),
  cancelDownload: (id: string) => invoke<void>('cancel_download', { id }),
  confirmClearHistory: () => confirm(
    'Clear download records, batch history, and logs? Downloaded files and their badges will be kept.',
    { title: 'Clear download history', kind: 'warning', okLabel: 'Clear history' },
  ),
  clearDownloadHistory: () => invoke<void>('clear_download_history'),
  removeDownload: (id: string) => invoke<void>('remove_download', { id }),
  downloadLogs: (id: string) => invoke<DownloadLog[]>('download_logs', { id }),
  syncDownloadAuthWindow: () => invoke<void>('sync_download_auth_window'),
  openDownloadLogs: () => invoke<void>('open_download_logs_window'),
  terminalSessions: () => invoke<TerminalSession[]>('terminal_sessions'),
  downloadActivity: () => invoke<DownloadActivity[]>('download_activity'),
  downloadPrompt: () => invoke<AuthPrompt | null>('download_prompt'),
  authenticateDownload: (id: string, response: string) => invoke<void>('authenticate_download', { id, response }),
  itemDownloadState: (id: string) => invoke<ItemDownloadState>('item_download_state', { id }),
  syncDownloadedBadges: () => invoke<void>('sync_downloaded_badges'),
  openDownloadFolder: (id: string, workshopItem = false) => invoke<void>('open_download_folder', { id, workshopItem }),
  downloadBackends: (settings: Settings, check = false) => invoke<BackendInfo[]>('download_backends', { settings, check }),
  backendRelease: (backend: BackendChoice) => invoke<BackendRelease>('backend_release', { backend }),
  installBackend: (backend: BackendChoice, source: string) => invoke<void>('install_backend', { backend, source }),
  restoreBackend: (backend: BackendChoice) => invoke<void>('restore_backend', { backend }),
  clearBackendSession: (backend: BackendChoice) => invoke<void>('clear_backend_session', { backend }),
  applicationInfo: () => invoke<ApplicationInfo>('application_info'),
  checkApplicationUpdate: () => invoke<UpdateInfo | null>('check_application_update'),
  installApplicationUpdate: (version: string) => invoke<void>('install_application_update', { version }),
  openApplicationReleases: () => invoke<void>('open_application_releases'),
  exportDiagnostics: async () => {
    const path = await save({ title: 'Export diagnostic activity', defaultPath: 'workshop-diagnostics.json',
      filters: [{ name: 'JSON', extensions: ['json'] }] });
    if (!path) return false;
    await invoke<void>('export_diagnostics', { path });
    return true;
  },
  openBackendSource: (backend: BackendChoice) => invoke<void>('open_backend_source', { backend }),
  selectExecutable: () => open({ title: 'Choose a downloader executable', directory: false, multiple: false }),
  selectFolder: (defaultPath?: string) =>
    open({ title: 'Choose a download directory', directory: true, multiple: false, defaultPath }),
};

export function errorMessage(error: unknown): string {
  return typeof error === 'string' ? error : error instanceof Error ? error.message : 'Something went wrong.';
}

export function parseAppId(input: string): number | null {
  const value = Number(input);
  return /^\d+$/.test(input.trim()) && Number.isInteger(value) && value > 0 && value <= 0xffffffff
    ? value
    : null;
}

export const backendNames: Record<BackendChoice, string> = {
  automatic: 'Automatic',
  steamcmd: 'SteamCMD',
  depotDownloader: 'DepotDownloader',
};

export function usesDepotQr(settings: Settings, backend: BackendChoice) {
  return backend === 'depotDownloader' && settings.depotQr;
}

export function signInAvailable(settings: Settings, backend: BackendChoice) {
  return usesDepotQr(settings, backend) || Boolean(settings.steamAccount.trim());
}

export function downloadOptions(settings: Settings, backend: BackendChoice, signedIn: boolean, replace = false): QueueOptions {
  const qr = signedIn && usesDepotQr(settings, backend);
  return { backend, replace, account: signedIn && !qr ? settings.steamAccount.trim() : null, qr };
}
