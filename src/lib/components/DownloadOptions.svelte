<script lang="ts">
  import { untrack } from 'svelte';
  import { backendNames, downloadOptions, signInAvailable, usesDepotQr } from '../desktop';
  import type { Settings } from '../contracts/Settings';
  import type { BackendChoice } from '../contracts/BackendChoice';
  import type { QueueOptions } from '../contracts/QueueOptions';

  let { backend, allowReplace = false, replace = false, signedIn = false, settings, busy, label, onSubmit, onClose }: {
    backend: BackendChoice;
    allowReplace?: boolean;
    replace?: boolean;
    signedIn?: boolean;
    settings: Settings;
    busy: boolean;
    label: string;
    onSubmit: (options: QueueOptions) => Promise<boolean>;
    onClose: () => void;
  } = $props();

  let selected = $state(untrack(() => backend));

  let useAccount = $state(untrack(() => signedIn));
  let replaceExisting = $state(untrack(() => replace));

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    await onSubmit(downloadOptions(settings, selected, useAccount, replaceExisting));
  }
</script>

<form class="download-options" onsubmit={submit}>
  <label>Backend <select aria-label="Download backend" bind:value={selected} disabled={busy}>
    {#each Object.entries(backendNames) as [id, name]}<option value={id}>{name}</option>{/each}
  </select></label>
  <label><input type="checkbox" bind:checked={useAccount} disabled={busy} /> Sign in with Steam</label>
  {#if useAccount}<span class="field-note">{usesDepotQr(settings, selected) ? 'Steam mobile app QR code' : settings.steamAccount || 'Set an account name in Preferences'}</span>{/if}
  {#if allowReplace}
    <label><input type="checkbox" bind:checked={replaceExisting} disabled={busy} /> Replace existing folder</label>
  {/if}
  <div class="download-option-actions">
    <button type="button" class="button" onclick={onClose} disabled={busy}>Close</button>
    <button type="submit" class="button primary" disabled={busy || (useAccount && !signInAvailable(settings, selected))}>{busy ? 'Working…' : label}</button>
  </div>
</form>
