<script lang="ts">
  import { untrack } from 'svelte';
  import { backendNames, contentLabels } from '../desktop';
  import type { WorkshopPreview } from '../contracts/WorkshopPreview';
  import type { Settings } from '../contracts/Settings';
  import type { QueueOptions } from '../contracts/QueueOptions';
  import type { ItemDownloadState } from '../contracts/ItemDownloadState';
  let { preview, settings, states, busy, onQueue, onClose }: {
    preview: WorkshopPreview; settings: Settings; states: Record<string, ItemDownloadState>; busy: boolean;
    onQueue: (selected: string[], options: QueueOptions, redownload: boolean) => Promise<void>; onClose: () => void;
  } = $props();
  function status(id: string) { return states[id]?.status ?? preview.items.find((entry) => entry.id === id)?.status ?? 'notDownloaded'; }
  function downloaded(id: string) { return ['downloaded', 'downloadedExternally'].includes(status(id)); }
  let redownload = $state(false);
  let selected = $state<string[]>(untrack(() => preview.items.filter((entry) => entry.item && !entry.unavailable && !downloaded(entry.id) && !['queued', 'downloading'].includes(status(entry.id))).map((entry) => entry.id)));
  let backend = $state(untrack(() => settings.backend));
  let signIn = $state(untrack(() => settings.signIn));
  const eligible = (id: string) => !['queued', 'downloading'].includes(status(id)) && (redownload || !downloaded(id));
  const count = $derived(selected.filter(eligible).length);
</script>

<section class="panel collection-preview" aria-label="Workshop download preview">
  <div class="panel-heading">
    <h2>{preview.name}</h2>
    <p class="preview-meta">{preview.appId ? settings.games.find((game) => game.appId === preview.appId)?.name ?? 'App ' + preview.appId : 'Workshop selection'} · {preview.items.length} unique items</p>
  </div>
  <div class="panel-body">
    {#each preview.warnings as warning}<p class="field-note error-text">{warning}</p>{/each}
    <div class="preview-actions">
      <button class="button" disabled={busy} onclick={() => selected = preview.items.filter((entry) => entry.item && !entry.unavailable && eligible(entry.id)).map((entry) => entry.id)}>Select All</button>
      <button class="button" disabled={busy} onclick={() => selected = []}>Deselect All</button>
      <label><input class="switch" type="checkbox" bind:checked={redownload} disabled={busy} /> Include downloaded items</label>
    </div>
    <div class="preview-items">
      {#each preview.items as entry (entry.id)}
        <label class="preview-item">
          <input class="preview-check" type="checkbox" value={entry.id} bind:group={selected} disabled={busy || !entry.item || !!entry.unavailable || !eligible(entry.id)} />
          <span><strong>{entry.item?.title ?? 'Workshop item #' + entry.id}</strong><small>#{entry.id}{entry.item ? ' · App ' + entry.item.appId : ''}</small></span>
          {#if entry.unavailable || status(entry.id) !== 'notDownloaded'}<small class:error-text={!!entry.unavailable}>{entry.unavailable ?? contentLabels[status(entry.id)]}</small>{/if}
        </label>
      {:else}<p class="muted">No downloadable items in this collection.</p>{/each}
    </div>
    <div class="preview-actions preview-footer">
      <label>Engine <select bind:value={backend} disabled={busy}>{#each Object.entries(backendNames) as [id, name]}<option value={id}>{name}</option>{/each}</select></label>
      <label><input class="switch" type="checkbox" bind:checked={signIn} disabled={busy} /> Sign in with Steam</label>
      {#if signIn}<span class="field-note">{settings.steamAccount || 'Set your Steam account name in Preferences first.'}</span>{/if}
      <button class="button" disabled={busy} onclick={onClose}>Cancel</button>
      <button class="button primary" disabled={busy || !count || (signIn && !settings.steamAccount.trim())}
        onclick={() => onQueue(selected.filter(eligible), { backend, account: signIn ? settings.steamAccount : null, replace: false }, redownload)}>
        {busy ? 'Queueing…' : `Download Selected (${count})`}
      </button>
    </div>
  </div>
</section>
