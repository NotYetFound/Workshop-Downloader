<script lang="ts">
  import { onMount } from 'svelte';
  import { confirm } from '@tauri-apps/plugin-dialog';
  import type { ApplicationInfo } from '../contracts/ApplicationInfo';
  import type { UpdateInfo } from '../contracts/UpdateInfo';
  import { desktop, errorMessage } from '../desktop';
  import Panel from './Panel.svelte';

  let { verbose = $bindable(), disabled, onSetup }: {
    verbose: boolean; disabled: boolean; onSetup: () => void;
  } = $props();
  let info = $state<ApplicationInfo | null>(null);
  let update = $state<UpdateInfo | null>(null);
  let busy = $state(false);
  let message = $state('');
  let error = $state('');

  async function perform(action: () => Promise<void>) {
    busy = true; message = ''; error = '';
    try { await action(); }
    catch (failure) { error = errorMessage(failure); }
    finally { busy = false; }
  }

  onMount(() => {
    void perform(async () => { info = await desktop.applicationInfo(); });
  });

  async function check() {
    update = null;
    await perform(async () => {
      update = await desktop.checkApplicationUpdate();
      if (!update) message = 'You have the latest version.';
    });
  }

  async function install(version: string) {
    await perform(async () => {
      if (!await confirm(`Install Workshop Downloader ${version} and restart?`,
        { title: 'Install update', okLabel: 'Install and restart' })) return;
      update = null;
      message = 'Downloading and verifying the update…';
      await desktop.installApplicationUpdate(version);
    });
  }
</script>

<Panel title="Application and diagnostics">
  {#if info}<p class="field-note">Workshop Downloader {info.version} · {info.os} · {info.architecture}</p>{/if}
  <div class="backend-actions">
    <button class="button" type="button" onclick={onSetup} disabled={busy}>Open setup</button>
    <button class="button" type="button" onclick={() => perform(desktop.openDownloadLogs)} disabled={busy}>Debug</button>
    {#if info?.updaterEnabled}
      <button class="button" type="button" onclick={check} disabled={busy || disabled}>Check for app updates</button>
    {/if}
    <button class="button" type="button" disabled={busy}
      onclick={() => perform(() => desktop.openApplicationReleases())}>GitHub Releases</button>
    <button class="button" type="button" disabled={busy} onclick={() => perform(async () => {
      if (await desktop.exportDiagnostics()) message = 'Diagnostics exported.';
    })}>Export diagnostics</button>
  </div>
  <label class="toggle-row">
    <span><strong>Verbose diagnostics</strong><small>Show additional backend lifecycle events in Debug.</small></span>
    <input type="checkbox" class="switch" role="switch" bind:checked={verbose} />
  </label>
  {#if info && !info.updaterEnabled}<p class="field-note">{info.updateMessage}</p>{/if}
  {#if update}
    {@const version = update.version}
    <p><strong>Version {version} is available</strong></p>
    {#if update.notes}<pre class="update-notes">{update.notes}</pre>{/if}
    <button class="button primary" type="button" onclick={() => install(version)} disabled={busy || disabled}>Install update</button>
  {/if}
  {#if message}<p class="field-note" role="status">{message}</p>{/if}
  {#if error}<p class="field-note error-text" role="alert">{error}</p>{/if}
</Panel>
