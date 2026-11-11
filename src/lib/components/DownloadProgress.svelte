<script lang="ts">
  import { Download } from '@lucide/svelte';
  import { formatBytes } from '../downloadDisplay';
  import type { DownloadJob } from '../contracts/DownloadJob';
  import type { DownloadStatus } from '../contracts/DownloadStatus';

  let { job, position, onOpen }: { job: DownloadJob; position: { index: number; total: number }; onOpen: () => void } = $props();
  const statusLabels: Record<DownloadStatus, string> = {
    queued: 'Queued', preparing: 'Preparing', authenticating: 'Signing in',
    downloading: 'Downloading', finalizing: 'Verifying files', completed: 'Complete',
    failed: 'Failed', cancelled: 'Cancelled',
  };
  const size = $derived(job.status === 'completed' ? job.downloadedBytes ?? job.expectedBytes : job.expectedBytes);
  const percent = $derived(job.status === 'completed' ? 100 : job.percent);
  const staged = $derived(job.provider === 'steamcmd' && job.status === 'downloading' && job.downloadedBytes !== null);
</script>

<section class="sidebar-progress" aria-label="Current download">
  <button type="button" class="sidebar-progress-title" onclick={onOpen} aria-label={'View downloads for ' + job.item.title} title={job.item.title}>
    <Download size={14} /><strong>{job.item.title}</strong>
  </button>
  <div class="sidebar-progress-meta">
    <span class="sidebar-progress-size" title={job.status === 'completed' ? 'Downloaded file size' : 'Size listed by Steam Workshop'}>{size === null ? 'Size unavailable' : formatBytes(size)}</span>
    <span class="sidebar-progress-count" aria-label={`Item ${position.index} of ${position.total}`}>{position.index}/{position.total}</span>
  </div>
  {#if percent !== null}
    <div class="sidebar-progress-track" role="progressbar" aria-valuemin={0} aria-valuemax={100} aria-valuenow={percent} aria-label={'Progress for ' + job.item.title}>
      {#key job.id}<div class="sidebar-progress-fill" style:width={`${percent}%`}></div>{/key}
    </div>
  {/if}
  <div class="sidebar-progress-status">
    <span>{statusLabels[job.status]}</span>
    {#if percent !== null}<span>{new Intl.NumberFormat(undefined, { maximumFractionDigits: 1 }).format(percent)}%</span>
    {:else if staged && job.downloadedBytes !== null}<span title="Bytes staged on disk">{formatBytes(job.downloadedBytes)} staged</span>{/if}
  </div>
</section>
