<script lang="ts">
  import { ChevronDown, Layers3 } from '@lucide/svelte';
  import DownloadsView from './DownloadsView.svelte';
  import { desktop, errorMessage } from '../desktop';
  import type { BatchProgress } from '../contracts/BatchProgress';
  import type { DownloadJob } from '../contracts/DownloadJob';
  import type { Settings } from '../contracts/Settings';
  import type { ItemDownloadState } from '../contracts/ItemDownloadState';
  let { batches, jobs, settings, states, onChanged, onOpen, onRetry, onCancel, onFolder, onRemove }: {
    batches: BatchProgress[]; jobs: DownloadJob[]; settings: Settings; states: Record<string, ItemDownloadState>;
    onChanged: () => Promise<void>; onOpen: (id: string) => void; onRetry: (job: DownloadJob) => void;
    onCancel: (id: string) => void; onFolder: (id: string) => void; onRemove: (id: string) => void;
  } = $props();
  let busy = $state(''); let error = $state('');
  async function action(id: string, retry: boolean) {
    busy = id; error = '';
    try {
      if (retry) await desktop.retryBatch(id, { backend: settings.backend, account: settings.signIn ? settings.steamAccount : null, replace: false });
      else await desktop.cancelBatch(id);
      await onChanged();
    } catch (failure) { error = errorMessage(failure); } finally { busy = ''; }
  }
</script>
<div class="batch-list">
  {#if error}<p class="error-text" role="alert">{error}</p>{/if}
  {#each batches as progress (progress.batch.id)}
    {@const batch = progress.batch}
    {@const members = jobs.filter((job) => batch.items.some((entry) => entry.jobId === job.id))}
    <details class="panel batch-panel">
      <summary>
        <span class="batch-avatar"><Layers3 size={20} strokeWidth={1.6} /></span>
        <span class="batch-title"><strong>{batch.name}</strong><small>{progress.total} items{batch.collectionId ? ' · Workshop collection' : ' · Workshop selection'}</small></span>
        <span class="batch-count"><strong>{progress.completed}/{progress.total} completed</strong><small>{[progress.failed ? progress.failed + ' failed' : '', progress.skipped ? progress.skipped + ' skipped' : '', progress.cancelled ? progress.cancelled + ' cancelled' : ''].filter(Boolean).join(' · ')}</small></span>
        <ChevronDown class="batch-chevron" size={18} />
      </summary>
      <div class="panel-body">
        <div class="preview-actions">
          <span class="field-note">{new Date(batch.createdAt).toLocaleString()}{batch.collectionId ? ' · Collection #' + batch.collectionId : ''}</span>
          {#if members.some((job) => ['queued', 'preparing', 'authenticating', 'downloading', 'finalizing'].includes(job.status))}<button class="button" disabled={!!busy} onclick={() => action(batch.id, false)}>Cancel remaining</button>{/if}
          {#if progress.failed || progress.cancelled || members.some((job) => job.status === 'queued' && job.accountRequired)}<button class="button" disabled={!!busy} onclick={() => action(batch.id, true)}>Retry failed / resume</button>{/if}
        </div>
        {#if members.length}<DownloadsView jobs={members} games={settings.games} {states} allowRemove={false} loading={false} failed={false} {onOpen} {onRetry} {onCancel} {onFolder} {onRemove} />{/if}
        {#each batch.items.filter((entry) => !entry.jobId) as entry}<p class="field-note">{entry.title} · {entry.reason ?? 'Skipped'}</p>{/each}
      </div>
    </details>
  {/each}
</div>
