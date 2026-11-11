<script lang="ts">
  import { Check, Download, ExternalLink, FileText, FolderOpen, Package, RotateCcw, Trash2, X } from '@lucide/svelte';
  import { onMount } from 'svelte';
  import { backendNames, contentLabels, desktop, errorMessage } from '../desktop';
  import { formatBytes } from '../downloadDisplay';
  import IconButton from './IconButton.svelte';
  import type { ItemDownloadState } from '../contracts/ItemDownloadState';
  import type { BackendChoice } from '../contracts/BackendChoice';
  import type { DownloadJob } from '../contracts/DownloadJob';
  import type { DownloadLog } from '../contracts/DownloadLog';
  import type { GamePreference } from '../contracts/GamePreference';

  let { jobs, games, states = {}, allowRemove = true, loading, failed, onOpen, onRetry, onCancel, onFolder, onRemove }: {
    jobs: DownloadJob[];
    allowRemove?: boolean;
    states?: Record<string, ItemDownloadState>;
    games: GamePreference[];
    loading: boolean;
    failed: boolean;
    onOpen: (id: string) => void;
    onRetry: (job: DownloadJob) => void;
    onCancel: (id: string) => void;
    onFolder: (id: string) => void;
    onRemove: (id: string) => void;
  } = $props();

  let expanded = $state<string | null>(null);
  let logs = $state<DownloadLog[]>([]);
  let logError = $state('');

  onMount(() => {
    const timer = window.setInterval(() => { if (expanded) void refreshLogs(expanded); }, 1500);
    return () => window.clearInterval(timer);
  });

  async function refreshLogs(id: string) {
    try {
      const next = await desktop.downloadLogs(id);
      if (expanded === id) { logs = next; logError = ''; }
    } catch (failure) {
      if (expanded === id) logError = errorMessage(failure);
    }
  }

  function toggleLogs(id: string) {
    expanded = expanded === id ? null : id;
    logs = [];
    logError = '';
    if (expanded) void refreshLogs(expanded);
  }

  function isActive(job: DownloadJob) {
    return ['queued', 'preparing', 'authenticating', 'downloading', 'finalizing'].includes(job.status);
  }

  function backendName(provider: string) {
    return backendNames[provider as BackendChoice] ?? provider;
  }
</script>

<div class="download-list" aria-label="Download records">
  {#if loading || failed || !jobs.length}
    <div class="download-empty" role="status">
      <Download size={28} strokeWidth={1.4} />
      <strong>{loading ? 'Loading downloads…' : failed ? 'Could not load downloads' : 'Your downloads will appear here'}</strong>
      <span>{failed ? 'Try opening Downloads again.' : 'Browse the Workshop and choose something to download.'}</span>
    </div>
  {:else}
    {#each jobs as job (job.id)}
      <article class="download-entry">
        <div class="download-row">
          <div class="download-avatar" class:complete={job.status === 'completed'}>
            {#if job.status === 'completed'}<Check size={20} strokeWidth={1.8} />{:else}<Package size={20} strokeWidth={1.6} />{/if}
          </div>
          <div class="download-identity">
            <strong>{job.item.title}</strong>
            <div class="download-meta">
              <span>{games.find((game) => game.appId === job.item.appId)?.name ?? 'App ' + job.item.appId}</span>
              <span>{backendName(job.provider)}</span>
              {#if job.downloadedBytes !== null}<span>{formatBytes(job.downloadedBytes)}</span>{/if}
              {#if states[job.item.id] && ['missingFiles', 'downloadedExternally'].includes(states[job.item.id].status)}
                <span class:warning-text={states[job.item.id].status === 'missingFiles'}>{contentLabels[states[job.item.id].status]}</span>
              {/if}
            </div>
            {#if job.destination}<code class="download-destination" title={job.destination}>{job.destination}</code>{/if}
          </div>
          <div class="download-progress">
            <span class="job-status {job.status}">{job.status}</span>
            {#if ['authenticating', 'downloading', 'finalizing'].includes(job.status)}
              {#if job.percent !== null}
                <progress aria-label={'Progress for ' + job.item.title} max="100" value={job.percent}></progress>
                <small>{job.percent.toFixed(1)}%{job.bytesPerSecond !== null ? ' · ' + formatBytes(job.bytesPerSecond) + '/s' : ''}</small>
              {:else}<progress aria-label={'Progress for ' + job.item.title}></progress>{/if}
            {/if}
          </div>
          <div class="job-actions">
            {#if isActive(job)}
              <IconButton label={'Cancel ' + job.item.title} onclick={() => onCancel(job.id)}><X size={16} /></IconButton>
              {#if job.status === 'queued' && job.accountRequired}
                <IconButton label={'Resume sign-in for ' + job.item.title} onclick={() => onRetry(job)}><RotateCcw size={16} /></IconButton>
              {/if}
            {:else}
              {#if job.status === 'completed'}
                <IconButton label={'Open folder for ' + job.item.title} onclick={() => onFolder(job.id)}><FolderOpen size={16} /></IconButton>
              {:else}
                <IconButton label={'Retry ' + job.item.title} onclick={() => onRetry(job)}><RotateCcw size={16} /></IconButton>
              {/if}
              {#if allowRemove}<IconButton label={'Remove history record for ' + job.item.title} onclick={() => onRemove(job.id)}><Trash2 size={16} /></IconButton>{/if}
            {/if}
            <IconButton label={'Logs for ' + job.item.title} onclick={() => toggleLogs(job.id)}><FileText size={16} /></IconButton>
            <IconButton label={'Open ' + job.item.title + ' in Workshop'} onclick={() => onOpen(job.item.id)}><ExternalLink size={16} /></IconButton>
          </div>
        </div>
        {#if job.error}<p class="download-error error-text">{job.error}</p>{/if}
        {#if expanded === job.id}
          <div class="download-log" aria-label={'Log for ' + job.item.title}>
            {#if logError}<p class="error-text">{logError}</p>
            {:else if logs.length === 0}<p class="muted">No log entries yet.</p>
            {:else}{#each logs as entry}<div><time datetime={entry.time}>{new Date(entry.time).toLocaleTimeString()}</time><span>{entry.message}</span></div>{/each}{/if}
          </div>
        {/if}
      </article>
    {/each}
  {/if}
</div>
