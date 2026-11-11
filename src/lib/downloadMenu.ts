import { Menu } from '@tauri-apps/api/menu';
import { LogicalPosition } from '@tauri-apps/api/dpi';
import type { BackendChoice } from './contracts/BackendChoice';
import { backendNames } from './desktop';

let active: Menu | null = null;
let revision = 0;

export async function closeDownloadMenu() {
  revision++;
  const previous = active;
  active = null;
  await previous?.close();
}

/** Native context menus paint above the Steam child webview on both platforms. */
export async function showDownloadMenu(x: number, y: number, signIn: boolean,
  download: (backend: BackendChoice, signIn: boolean) => void, failed: (error: unknown) => void) {
  const closing = closeDownloadMenu();
  const sequence = revision;
  await closing;
  const position = new LogicalPosition(x, y);
  let useAccount = signIn;
  const menu = await Menu.new({ items: [
    { text: 'Sign in with Steam', checked: signIn, action: () => {
      if (active !== menu) return;
      useAccount = !useAccount;
      // Native menus dismiss check items; reopen so an engine can be picked next.
      window.setTimeout(() => {
        if (active === menu) void menu.popup(position).catch(failed);
      }, 0);
    } },
    { item: 'Separator' },
    ...Object.entries(backendNames).map(([backend, text]) => ({
      text, action: () => {
        if (active !== menu) return;
        download(backend as BackendChoice, useAccount);
        void closeDownloadMenu().catch(failed);
      },
    })),
  ] });
  if (sequence !== revision) { await menu.close(); return; }
  active = menu;
  await menu.popup(position);
}
