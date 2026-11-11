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
  const size = $derived(job.downloadedBytes ?? job.expectedBytes);
  const percent = $derived(job.status === 'completed' ? 100 : job.percent);
  const transferring = $derived(job.status === 'downloading' || job.status === 'finalizing');
</script>

<section class="sidebar-progress" aria-label="Current download">
  <button type="button" class="sidebar-progress-title" onclick={onOpen} aria-label={'View downloads for ' + job.item.title} title={job.item.title}>
    <Download size={14} /><strong>{job.item.title}</strong>
  </button>
  <div class="sidebar-progress-meta">
    <span class="sidebar-progress-size" title={job.downloadedBytes === null ? 'Size listed by Steam Workshop' : 'Size reported by the downloader'}>{size === null ? 'Size unavailable' : formatBytes(size)}</span>
    <span class="sidebar-progress-count" aria-label={`Item ${position.index} of ${position.total}`}>{position.index}/{position.total}</span>
  </div>
  <div class="sidebar-progress-track" role="progressbar" aria-valuemin={0} aria-valuemax={100} aria-valuenow={percent ?? undefined}
    aria-valuetext={percent === null ? statusLabels[job.status] : undefined} aria-label={'Progress for ' + job.item.title}>
    {#key job.id}
      <div class="sidebar-progress-fill" class:indeterminate={percent === null && transferring} style:width={`${percent ?? 0}%`}></div>
    {/key}
  </div>
  <div class="sidebar-progress-status">
    <span>{statusLabels[job.status]}</span>
    {#if percent !== null}<span>{new Intl.NumberFormat(undefined, { maximumFractionDigits: 1 }).format(percent)}%</span>{/if}
  </div>
</section>
