import type { DownloadJob } from './contracts/DownloadJob';
import type { DownloadStatus } from './contracts/DownloadStatus';

export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const unit = Math.min(4, Math.floor(Math.log(bytes) / Math.log(1024)));
  return `${(bytes / 1024 ** unit).toFixed(1)} ${['B', 'KiB', 'MiB', 'GiB', 'TiB'][unit]}`;
}

const finished = (status: DownloadStatus) => status === 'completed' || status === 'failed' || status === 'cancelled';

export function updateQueueOrder(
  previousOrder: string[],
  jobs: Pick<DownloadJob, 'id' | 'status'>[],
  previousStatuses: ReadonlyMap<string, DownloadStatus>,
): string[] {
  const pending = [...jobs].reverse().filter((job) => !finished(job.status));
  const hadPending = [...previousStatuses.values()].some((status) => !finished(status));
  if (!pending.length && !hadPending) return previousOrder;
  if (!hadPending || !previousOrder.length) {
    const active = pending.find((job) => job.status !== 'queued');
    return active
      ? [active.id, ...pending.filter((job) => job.id !== active.id).map((job) => job.id)]
      : pending.map((job) => job.id);
  }

  const present = new Set(jobs.map((job) => job.id));
  const tracked = new Set(previousOrder);
  const added = new Set(pending.filter((job) => {
    const oldStatus = previousStatuses.get(job.id);
    return !tracked.has(job.id) || (oldStatus !== undefined && finished(oldStatus));
  }).map((job) => job.id));
  return [...previousOrder.filter((id) => present.has(id) && !added.has(id)), ...added];
}
