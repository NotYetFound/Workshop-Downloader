<script lang="ts">
  import { untrack } from 'svelte';
  import { backendNames } from '../desktop';
  import type { BackendChoice } from '../contracts/BackendChoice';
  import type { QueueOptions } from '../contracts/QueueOptions';

  let { backend, allowReplace = false, replace = false, signedIn = false, accountName = '', busy, label, onSubmit, onClose }: {
    backend: BackendChoice;
    allowReplace?: boolean;
    replace?: boolean;
    signedIn?: boolean;
    accountName?: string;
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
    if (await onSubmit({ backend: selected, replace: replaceExisting, account: useAccount ? accountName.trim() : null })) {

    }
  }
</script>

<form class="download-options" onsubmit={submit}>
  <label>Backend <select aria-label="Download backend" bind:value={selected} disabled={busy}>
    {#each Object.entries(backendNames) as [id, name]}<option value={id}>{name}</option>{/each}
  </select></label>
  <label><input type="checkbox" bind:checked={useAccount} disabled={busy} /> Sign in with Steam</label>
  {#if useAccount}<span class="field-note">Account from Preferences: {accountName || 'Set an account name first'}</span>{/if}
  {#if allowReplace}
    <label><input type="checkbox" bind:checked={replaceExisting} disabled={busy} /> Replace existing folder</label>
  {/if}
  <div class="download-option-actions">
    <button type="button" class="button" onclick={onClose} disabled={busy}>Close</button>
    <button type="submit" class="button primary" disabled={busy || (useAccount && !accountName.trim())}>{busy ? 'Working…' : label}</button>
  </div>
</form>
