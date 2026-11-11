<script lang="ts">
  import { onMount } from 'svelte';
  import Panel from './Panel.svelte';
  import { desktop, errorMessage } from '../desktop';
  import type { ExternalSummary } from '../contracts/ExternalSummary';
  import type { ScanReport } from '../contracts/ScanReport';
  import type { ItemDownloadState } from '../contracts/ItemDownloadState';
  let { onImported }: { states: Record<string, ItemDownloadState>; onImported: () => void } = $props();
  let items = $state<ExternalSummary[]>([]); let directory = $state(''); let workshopUrl = $state('');
  let report = $state<ScanReport | null>(null); let busy = $state(false); let error = $state('');
  onMount(() => {
    const refresh = () => { void desktop.externalItems().then((value) => items = value).catch((failure) => error = errorMessage(failure)); };
    refresh(); const timer = window.setInterval(refresh, 5000);
    return () => window.clearInterval(timer);
  });
  async function choose() {
    try { const path = await desktop.selectFolder(directory || undefined); if (path) directory = path; }
    catch (failure) { error = errorMessage(failure); }
  }
  async function scan() {
    busy = true; error = ''; report = null;
    try {
      report = await desktop.scanExternal(directory, workshopUrl || null);
      items = await desktop.externalItems(); onImported();
    } catch (failure) { error = errorMessage(failure); } finally { busy = false; }
  }
</script>
<Panel title="Existing downloads" description="Recognize content you already have, right where it is.">
  <div class="field-row"><input aria-label="External download directory" bind:value={directory} placeholder="Select a Steam Workshop or DepotDownloader directory" disabled={busy} /><button class="button" onclick={choose} disabled={busy}>Browse</button></div>
  <div class="field-row"><input aria-label="DepotDownloader Workshop URL" bind:value={workshopUrl} placeholder="Optional: matching Workshop item URL for DepotDownloader output" disabled={busy} /><button class="button" onclick={scan} disabled={busy || !directory}>{busy ? 'Scanning…' : 'Scan directory'}</button></div>
  <p class="field-note">Reads manifests and verifies files. Steam-managed and external files stay in their original location and are never changed.</p>
  {#if error}<p class="error-text" role="alert">{error}</p>{/if}
  {#if report}
    <p class="field-note" role="status">Recognized {report.recognized} items; added {report.imported} new records.</p>
    {#if report.warnings.length}<details><summary>{report.warnings.length} scan notes</summary>{#each report.warnings as warning}<p class="field-note">{warning}</p>{/each}</details>{/if}
  {/if}
  {#each items as record (record.item.id + record.path)}
    <div class="external-record"><strong>{record.item.title}</strong><span>{record.source} · {record.verified ? 'Downloaded externally' : 'Missing files'}</span><code title={record.path}>{record.path}</code>
      <button class="button" disabled={!record.verified} onclick={() => desktop.openExternalFolder(record.item.id, record.path).catch((failure) => error = errorMessage(failure))}>Open folder</button>
    </div>
  {/each}
</Panel>
