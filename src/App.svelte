<script lang="ts">
  import { CircleAlert, Box, Compass, Download, FolderOpen, Layers3, Settings2, X } from '@lucide/svelte';
  import WorkshopPreview from './lib/components/WorkshopPreview.svelte';
  import BatchView from './lib/components/BatchView.svelte';
  import type { WorkshopPreview as PreviewModel } from './lib/contracts/WorkshopPreview';
  import type { BatchProgress } from './lib/contracts/BatchProgress';
  import { onMount, tick } from 'svelte';
  import BrowserBar from './lib/components/BrowserBar.svelte';
  import DownloadProgress from './lib/components/DownloadProgress.svelte';
  import DownloadOptions from './lib/components/DownloadOptions.svelte';
  import DownloadsView from './lib/components/DownloadsView.svelte';
  import IconButton from './lib/components/IconButton.svelte';
  import SettingsView from './lib/components/SettingsView.svelte';
  import { closeDownloadMenu, showDownloadMenu } from './lib/downloadMenu';
  import { contentLabels, desktop, downloadOptions, errorMessage, signInAvailable } from './lib/desktop';
  import type { BackendChoice } from './lib/contracts/BackendChoice';
  import type { ItemDownloadState } from './lib/contracts/ItemDownloadState';
  import { updateQueueOrder } from './lib/downloadDisplay';
  import type { QueueOptions } from './lib/contracts/QueueOptions';
  import type { BrowserAction } from './lib/contracts/BrowserAction';
  import type { BrowserState } from './lib/contracts/BrowserState';
  import type { DownloadStatus } from './lib/contracts/DownloadStatus';
  import type { DownloadJob } from './lib/contracts/DownloadJob';
  import type { ItemInspection } from './lib/contracts/ItemInspection';
  import type { Settings } from './lib/contracts/Settings';

  type Page = 'browse' | 'downloads' | 'settings';
  let page = $state<Page>('browse');
  let settings = $state<Settings | null>(null);
  let setup = $state(false);
  let browser = $state<BrowserState | null>(null);
  let jobs = $state<DownloadJob[]>([]);
  let batches = $state<BatchProgress[]>([]);
  let states = $state<Record<string, ItemDownloadState>>({});
  let preview = $state<PreviewModel | null>(null);
  let previewLoading = $state(false);
  let singleRequest = $state<{ id: string; title: string; previewId: string } | null>(null);
  let singleLoading = $state(false);
  let previewError = $state('');
  let previewGeneration = 0;
  let previewOptions = $state<{ backend: BackendChoice; signIn: boolean } | null>(null);
  let collection = $state<boolean | null>(null);
  const activeJobCount = $derived(jobs.filter((job) => ['queued', 'preparing', 'authenticating', 'downloading', 'finalizing'].includes(job.status)).length);
  const standaloneJobs = $derived(jobs.filter((job) => !batches.some((batch) => batch.batch.items.some((entry) => entry.jobId === job.id))));

  function closePreview() {
    previewGeneration++;
    preview = null;
    singleRequest = null;
    singleLoading = false;
    previewLoading = false;
    previewError = '';
  }
  async function openPreview(collectionId: string | null, ids: string[] = [], override?: { backend: BackendChoice; signIn: boolean }) {
    if (previewLoading || downloadBusy) return;
    await closeDownloadMenu();
    const generation = ++previewGeneration;
    singleRequest = null;
    singleLoading = false;
    previewOptions = override ?? null;
    preview = null;
    previewError = '';
    previewLoading = true;
    try {
      const result = await desktop.previewWorkshop(collectionId, ids);
      if (generation === previewGeneration) preview = result;
    } catch (failure) {
      if (generation === previewGeneration) previewError = errorMessage(failure);
    } finally {
      if (generation === previewGeneration) previewLoading = false;
    }
  }
  async function requestSingleItem(id: string) {
    if (singleLoading || downloadBusy) return;
    const generation = ++previewGeneration;
    singleRequest = null;
    singleLoading = true;
    try {
      const result = await desktop.previewWorkshop(null, [id]);
      if (generation !== previewGeneration || page !== 'browse') return;
      const entry = result.items[0];
      if (entry?.item && !entry.unavailable) {
        singleRequest = { id: entry.id, title: entry.item.title, previewId: result.id };
      } else if (await desktop.isCollection(id)) {
        if (generation === previewGeneration && page === 'browse') await openPreview(id);
      } else {
        showDownloadError(entry?.unavailable ?? 'Workshop item is unavailable.');
      }
    } catch (failure) {
      if (generation === previewGeneration) showDownloadError(errorMessage(failure));
    } finally {
      if (generation === previewGeneration) singleLoading = false;
    }
  }

  async function confirmSingleItem() {
    if (!singleRequest || !settings || downloadBusy) return;
    if (settings.signIn && !signInAvailable(settings, settings.backend)) {
      showDownloadError('Enter your Steam account name in Preferences.');
      await showPage('settings');
      return;
    }
    const request = singleRequest;
    const options = downloadOptions(settings, settings.backend, settings.signIn);
    downloadBusy = true;
    try {
      await desktop.queueDownload(
        'https://steamcommunity.com/sharedfiles/filedetails/?id=' + encodeURIComponent(request.id), options, request.previewId,
      );
      if (singleRequest?.previewId === request.previewId) closePreview();
      await refreshDownloads();
      await refreshBadges();
    } catch (failure) {
      showDownloadError(errorMessage(failure));
    } finally {
      downloadBusy = false;
    }
  }

  async function queuePreview(selected: string[], options: QueueOptions, redownload: boolean) {
    if (!preview || downloadBusy) return;
    const generation = previewGeneration;
    downloadBusy = true;
    previewError = '';
    try {
      await desktop.queueBatch(preview.id, selected, options, redownload);
      if (generation === previewGeneration) closePreview();
      await refreshDownloads();
      await refreshItemDownload();
    } catch (failure) {
      if (generation === previewGeneration) previewError = errorMessage(failure);
      else showDownloadError(errorMessage(failure));
    }
    finally { downloadBusy = false; }
  }
  async function detectCollection(id: string) {
    try {
      const result = await desktop.isCollection(id);
      if (browser?.itemId === id) collection = result;
    } catch { /* The download action retries metadata and reports actionable errors. */ }
  }

  let jobsLoading = $state(false);
  let jobsFailed = $state(false);
  let itemDownload = $state<ItemDownloadState | null>(null);
  let requestedItem = $state<string | null>(null);
  let retryJob = $state<DownloadJob | null>(null);
  let downloadBusy = $state(false);
  let downloadAlert = $state<{ title: string; detail: string } | null>(null);
  let recentCompletion = $state<DownloadJob | null>(null);
  let completionTimer: number | undefined;
  const currentDownload = $derived(
    jobs.find((job) => ['preparing', 'authenticating', 'downloading', 'finalizing'].includes(job.status))
      ?? [...jobs].reverse().find((job) => job.status === 'queued')
      ?? recentCompletion,
  );
  let queueOrder = $state<string[]>([]);
  const downloadPosition = $derived({
    index: Math.max(0, queueOrder.indexOf(currentDownload?.id ?? '')) + 1,
    total: Math.max(1, queueOrder.length),
  });

  let jobStatuses = new Map<string, DownloadStatus>();
  let haveJobSnapshot = false;

  function showDownloadError(detail: string, title = 'Download issue') {
    downloadAlert = { title, detail };
  }

  function showCompletion(job: DownloadJob) {
    recentCompletion = job;
    window.clearTimeout(completionTimer);
    completionTimer = window.setTimeout(() => recentCompletion = null, 4000);
  }

  function openDownloadMenu(event: MouseEvent) {
    event.preventDefault();
    if (!settings || !browser?.itemId) return;
    const id = browser.itemId;
    const button = event.currentTarget as HTMLButtonElement;
    const rect = button.getBoundingClientRect();
    void showDownloadMenu(rect.right - 190, rect.bottom, settings.signIn,
      (backend, signIn) => {
        if (browser?.itemId === id) void downloadNow({ backend, signIn });
      }, (failure) => showDownloadError(errorMessage(failure))).catch((failure) => showDownloadError(errorMessage(failure)));
  }

  async function downloadNow(override?: { backend: BackendChoice; signIn: boolean }) {
    if (!settings || !browser?.itemId || itemDownload?.active || downloadBusy) return;
    const id = browser.itemId;
    if (collection === null) {
      try {
        const result = await desktop.isCollection(id);
        if (browser?.itemId !== id) return;
        collection = result;
      } catch (failure) { showDownloadError(errorMessage(failure)); return; }
    }
    if (collection) { await openPreview(id, [], override); return; }
    const signIn = override?.signIn ?? settings.signIn;
    const backend = override?.backend ?? settings.backend;
    if (signIn && !signInAvailable(settings, backend)) {
      showDownloadError('Enter your Steam account name in Preferences.');
      await showPage('settings');
      return;
    }
    requestedItem = browser.itemId;
    await queueDownload(downloadOptions(settings, backend, signIn));
  }
  let checkingDownloads = false;
  let historyRevision = 0;
  let checkingItem = false;
  let checkingBadges = false;
  let address = $state('');
  let editingAddress = $state(false);
  let error = $state('');
  let startupError = $state('');
  let navigating = $state(false);
  let inspecting = $state(false);
  let inspection = $state<ItemInspection | null>(null);
  let slotElement = $state<HTMLElement>();
  let lastUrl: string | null = null;
  let synchronizing = false;
  let slotUpdate = Promise.resolve();
  let slotRevision = 0;

  async function refreshBrowser() {
    if (synchronizing) return;
    synchronizing = true;
    try {
      const next = await desktop.browserState();
      if (next.url !== lastUrl) {
        if (singleRequest || singleLoading) closePreview();
        if (!editingAddress && next.url) address = next.url;
        if (next.itemId !== browser?.itemId) {
          collection = null;
          inspection = null;
          itemDownload = null;
          requestedItem = null;
          void closeDownloadMenu();
        }
        lastUrl = next.url;
      }
      const itemChanged = next.itemId !== browser?.itemId;
      browser = next;
      const pending = await desktop.takeWorkshopPreview();
      if (pending?.length && page === 'browse' && !preview && !previewLoading) {
        const ids = [...new Set(pending)];
        if (ids.length === 1) void requestSingleItem(ids[0]);
        else void openPreview(null, ids);
      }
      if (itemChanged) {
        void refreshItemDownload();
        if (next.itemId) void detectCollection(next.itemId);
      }
    } finally {
      synchronizing = false;
    }
  }

  function updateSlot(node: HTMLElement | undefined) {
    const revision = ++slotRevision;
    const rect = node?.getBoundingClientRect();
    const bounds = rect && rect.width >= 1 && rect.height >= 1
      ? { x: rect.x, y: rect.y, width: rect.width, height: rect.height }
      : null;
    slotUpdate = slotUpdate.then(async () => {
      if (revision !== slotRevision) return;
      await desktop.setBrowserSlot(bounds);
      if (bounds && !browser?.open) await refreshBrowser();
    }).catch((failure) => {
      error = errorMessage(failure);
    });
    return slotUpdate;
  }

  onMount(() => {
    void initialize();
    const timer = window.setInterval(() => {
      if (page === 'browse' && browser?.open && !navigating) {
        void refreshBrowser().catch((failure) => error = errorMessage(failure));
      }
    }, 500);
    const downloadsTimer = window.setInterval(() => void refreshDownloads(), 750);
    const badgeTimer = window.setInterval(() => {
      if (settings) void refreshBadges();
    }, 2300);
    return () => {
      void closeDownloadMenu();
      window.clearInterval(timer);
      window.clearInterval(downloadsTimer);
      window.clearTimeout(completionTimer);
      window.clearInterval(badgeTimer);
    };
  });

  $effect(() => {
    const node = page === 'browse' && !preview && !previewLoading && !previewError ? slotElement : undefined;
    if (!settings) return;
    const update = () => void updateSlot(node);
    update();
    if (!node) return;

    const observer = new ResizeObserver(update);
    observer.observe(node);
    window.addEventListener('resize', update);
    let scale = window.matchMedia(`(resolution: ${window.devicePixelRatio}dppx)`);
    function displayChanged() {
      scale.removeEventListener('change', displayChanged);
      scale = window.matchMedia(`(resolution: ${window.devicePixelRatio}dppx)`);
      scale.addEventListener('change', displayChanged);
      update();
    }
    scale.addEventListener('change', displayChanged);
    return () => {
      observer.disconnect();
      window.removeEventListener('resize', update);
      scale.removeEventListener('change', displayChanged);
    };
  });

  async function initialize() {
    startupError = '';
    try {
      settings = await desktop.getSettings();
      if (!settings.setupComplete) {
        setup = true;
        page = 'settings';
      }
    } catch (failure) {
      startupError = errorMessage(failure);
    }
  }

  async function go(task: () => Promise<void>) {
    if (navigating) return;
    closePreview();
    navigating = true;
    error = '';
    try {
      await task();
      await refreshBrowser();
    } catch (failure) {
      error = errorMessage(failure);
    } finally {
      navigating = false;
    }
  }

  function browserAction(action: BrowserAction) {
    const hasPrompt = preview || previewLoading || previewError || singleRequest || singleLoading;
    if (hasPrompt) {
      closePreview();
      if (action === 'back') return;
    }
    void go(() => desktop.browserAction(action));
  }

  function browseGame(appId: number) {
    void go(() => desktop.navigate({ kind: 'game', value: appId }));
  }

  function browseAddress(value: string) {
    return go(() => desktop.navigate({ kind: 'url', value }));
  }

  async function inspect() {
    const url = browser?.url;
    if (!url || !browser?.itemId || inspecting) return;
    inspecting = true;
    inspection = null;
    error = '';
    try {
      const result = await desktop.inspectItem(url);
      if (browser?.itemId === result.item.id) inspection = result;
    } catch (failure) {
      error = errorMessage(failure);
    } finally {
      inspecting = false;
    }
  }

  async function refreshDownloads() {
    if (checkingDownloads || !settings) return;
    checkingDownloads = true;
    const revision = historyRevision;
    try {
      await desktop.syncDownloadAuthWindow().catch((failure) => showDownloadError(errorMessage(failure)));
      const nextJobs = await desktop.downloads();
      if (revision !== historyRevision) return;
      if (haveJobSnapshot) {
        const changed = nextJobs.filter((job) => jobStatuses.get(job.id) !== job.status);
        const completed = changed.find((job) => job.status === 'completed');
        if (completed) showCompletion(completed);
        const failed = changed.find((job) => job.status === 'failed');
        if (failed) showDownloadError(failed.error ?? 'The engine stopped before the download completed.', failed.item.title);
      }
      queueOrder = updateQueueOrder(queueOrder, nextJobs, jobStatuses);
      jobStatuses = new Map(nextJobs.map((job) => [job.id, job.status]));
      haveJobSnapshot = true;
      jobs = nextJobs;
      jobsFailed = false;
      if (page === 'downloads') {
        const nextBatches = await desktop.batches();
        if (revision === historyRevision) batches = nextBatches;
      }
    } catch (failure) {
      if (page === 'downloads') {
        jobsFailed = true;
        error = errorMessage(failure);
      }
    } finally {
      jobsLoading = false;
      checkingDownloads = false;
    }
  }

  async function refreshItemDownload() {
    const id = browser?.itemId;
    if (!id || checkingItem) return;
    checkingItem = true;
    try {
      const next = await desktop.itemDownloadState(id);
      if (browser?.itemId === id) itemDownload = next;
    } catch (failure) {
      error = errorMessage(failure);
    } finally {
      checkingItem = false;
    }
  }

  async function refreshBadges() {
    if (checkingBadges) return;
    checkingBadges = true;
    const revision = historyRevision;
    try {
      const next = await desktop.contentStates();
      if (revision !== historyRevision) return;
      states = next;
      if (browser?.itemId) itemDownload = next[browser.itemId] ?? null;
      if (page === 'browse' && browser?.open && !browser.interrupted) await desktop.syncDownloadedBadges();
    } catch (failure) {
      error = errorMessage(failure);
    } finally {
      checkingBadges = false;
    }
  }

  async function showPage(next: Page) {
    if (next === page) return;
    closePreview();
    if (next !== 'settings') setup = false;
    page = next;
    error = '';
    void closeDownloadMenu();
    retryJob = null;
    requestedItem = null;
    if (next === 'downloads') {
      jobsLoading = !jobs.length;
      await refreshDownloads();
    }
  }

  async function queueDownload(options: QueueOptions) {
    const url = browser?.url;
    if (!url || !requestedItem || requestedItem !== browser?.itemId || downloadBusy) return false;
    downloadBusy = true;
    error = '';
    try {
      await desktop.queueDownload(url, options);
      requestedItem = null;
      await refreshItemDownload();
      return true;
    } catch (failure) {
      showDownloadError(errorMessage(failure));
      return false;
    } finally {
      downloadBusy = false;
    }
  }

  async function retryDownload(options: QueueOptions) {
    if (!retryJob || downloadBusy) return false;
    downloadBusy = true;
    error = '';
    try {
      await desktop.retryDownload(retryJob.id, options);
      retryJob = null;
      await refreshDownloads();
      return true;
    } catch (failure) {
      showDownloadError(errorMessage(failure));
      return false;
    } finally {
      downloadBusy = false;
    }
  }

  let clearingHistory = $state(false);

  async function clearHistory() {
    if (clearingHistory || activeJobCount) return;
    clearingHistory = true;
    try {
      if (!await desktop.confirmClearHistory()) return;
      await desktop.clearDownloadHistory();
      historyRevision++;
      jobStatuses.clear();
      retryJob = null;
      downloadAlert = null;
      recentCompletion = null;
      jobs = [];
      batches = [];
      await refreshDownloads();
    } catch (failure) {
      showDownloadError(errorMessage(failure));
    } finally {
      clearingHistory = false;
    }
  }

  async function jobAction(action: () => Promise<void>) {
    error = '';
    try {
      await action();
      await refreshDownloads();
    } catch (failure) {
      error = errorMessage(failure);
    }
  }

  function cancelDownload(id: string) {
    void jobAction(() => desktop.cancelDownload(id));
  }

  async function openRecordedItem(id: string) {
    await showPage('browse');
    await tick();
    await updateSlot(slotElement);
    await browseAddress('https://steamcommunity.com/sharedfiles/filedetails/?id=' + encodeURIComponent(id));
  }

  function updateSettings(next: Settings) {
    settings = next;
    if (setup) { setup = false; void showPage('browse'); }
    void closeDownloadMenu();
    inspection = null;
  }
</script>

<div class="desktop">
  <aside class="sidebar" aria-label="Application">
    <div class="brand"><Box size={18} strokeWidth={1.8} /><strong>Workshop</strong></div>
    <nav aria-label="Workspace">
      <button class:active={page === 'browse'} aria-current={page === 'browse' ? 'page' : undefined} onclick={() => showPage('browse')}><Compass size={16} /> Browse</button>
      <button class:active={page === 'downloads'} aria-current={page === 'downloads' ? 'page' : undefined} onclick={() => showPage('downloads')}><Download size={16} /> Downloads</button>
      <button class:active={page === 'settings'} aria-current={page === 'settings' ? 'page' : undefined} onclick={() => showPage('settings')}><Settings2 size={16} /> Preferences</button>
    </nav>
    {#if currentDownload || downloadAlert}
      <div class="sidebar-downloads">
        {#if currentDownload}
          <DownloadProgress job={currentDownload} position={downloadPosition} onOpen={() => showPage('downloads')} />
        {/if}
        {#if downloadAlert}
          <section class="sidebar-download-error" aria-label="Download error">
            <div class="sidebar-error-heading">
              <CircleAlert size={14} />
              <strong>Download error</strong>
              <IconButton label="Dismiss download error" onclick={() => downloadAlert = null}><X size={14} /></IconButton>
            </div>
            <div role="alert"><strong>{downloadAlert.title}</strong><p>{downloadAlert.detail}</p></div>
            <button type="button" class="sidebar-error-link" onclick={() => showPage('downloads')}>View downloads</button>
          </section>
        {/if}
      </div>
    {/if}
  </aside>

  <main class="workspace">
    {#if startupError}
      <div class="notice error" role="alert">
        <span>{startupError}</span>
        <button type="button" class="button" onclick={initialize}>Retry</button>
      </div>
    {:else if !settings}
      <div class="notice" role="status">Loading Workshop Downloader…</div>
    {:else}
      {#if page === 'browse'}
        <BrowserBar games={settings.games} appId={inspection?.item.appId ?? browser?.appId ?? null}
          bind:address bind:editing={editingAddress} disabled={navigating || !browser?.open}
          onAction={browserAction} onAddress={browseAddress} onGame={browseGame} onManage={() => showPage('settings')} />
      {:else}
        <header class="workspace-header"><h1>{page === 'downloads' ? 'Downloads' : setup ? 'Setup' : 'Preferences'}</h1></header>
      {/if}

      {#if error}
        <div class="notice error" role="alert">
          <span>{error}</span>
          <IconButton label="Dismiss error" onclick={() => error = ''}><X size={15} /></IconButton>
        </div>
      {/if}

      {#if page === 'browse'}
        {#if singleRequest || singleLoading}
          <div class="browser-subbar single-download">
            <Download size={15} />
            <span>{singleRequest?.title ?? 'Finding Workshop item…'}</span>
            {#if singleRequest}<button type="button" class="button primary" onclick={confirmSingleItem} disabled={downloadBusy}>Download</button>{/if}
            <IconButton label="Dismiss download" onclick={closePreview}><X size={15} /></IconButton>
          </div>
        {/if}
        {#if browser?.itemId}
          <div class="browser-subbar item-strip">
            <Layers3 size={15} />
            <span>Workshop item <code>#{browser.itemId}</code></span>
            {#if itemDownload?.downloaded}
              <span class="local-downloaded">{itemDownload.status === 'downloadedExternally' ? 'Downloaded externally' : 'Downloaded'}</span>
              <IconButton label="Open downloaded item folder" onclick={() => jobAction(() => desktop.openDownloadFolder(browser?.itemId ?? '', true))}><FolderOpen size={15} /></IconButton>
            {/if}
            {#if itemDownload && !itemDownload.downloaded && itemDownload.status !== 'notDownloaded'}<span class="muted">{contentLabels[itemDownload.status]}</span>{/if}
            <button class="button" type="button" onclick={inspect} disabled={inspecting}>
              {inspecting ? 'Resolving…' : inspection ? 'Refresh details' : 'Inspect item'}
            </button>
            <button id="download-trigger" class="button primary" type="button" disabled={itemDownload?.active || downloadBusy}
              onclick={() => downloadNow()} oncontextmenu={openDownloadMenu}>
              <Download size={15} /> {collection ? 'Download Collection…' : itemDownload?.active ? 'In queue' : itemDownload?.downloaded ? 'Download again…' : 'Download…'}
            </button>
          </div>

        {/if}

        {#if inspection}
          <div class="inspection">
            <div>
              <strong>{inspection.item.title}</strong>
              <span>{inspection.gameName} · App {inspection.item.appId}</span>
              <code title="Planned destination">{inspection.destination}</code>
            </div>
            <IconButton label="Close item details" onclick={() => inspection = null}><X size={15} /></IconButton>
          </div>
        {/if}

        {#if preview || previewLoading || previewError}
          <div class="scroll-page preview-page">
            {#if previewLoading}<div class="panel"><div class="panel-body"><span role="status">Resolving Workshop metadata…</span><button class="button" onclick={closePreview}>Cancel</button></div></div>{/if}
            {#if previewError}<p class="error-text" role="alert">{previewError}</p>{#if !preview}<button class="button" onclick={closePreview}>Close preview</button>{/if}{/if}
            {#if preview && settings}
              {#key preview.id}<WorkshopPreview {preview} settings={{ ...settings, backend: previewOptions?.backend ?? settings.backend, signIn: previewOptions?.signIn ?? settings.signIn }} {states} busy={downloadBusy} onQueue={queuePreview} onClose={closePreview} />{/key}
            {/if}
          </div>
        {/if}
        <div class="browser-slot" class:covered={!!preview || previewLoading || !!previewError} bind:this={slotElement}>
          {#if !browser?.open || browser.interrupted}
            <div class="browser-placeholder">
              {#if browser?.interrupted}
                <span role="status">The Steam webview was interrupted.</span>
                <button class="button" type="button" onclick={() => browserAction('refresh')}>Reload Steam Workshop</button>
              {:else if error}
                <button class="button" type="button" onclick={() => { error = ''; void updateSlot(slotElement); }}>Retry loading Steam Workshop</button>
              {:else}
                <span role="status">Opening Steam Workshop…</span>
              {/if}
            </div>
          {/if}
        </div>
      {:else if page === 'downloads'}
        <div class="scroll-page downloads-page">
          <div class="downloads-overview">
            <p>{jobs.length} {jobs.length === 1 ? 'item' : 'items'} in your history{batches.length ? ' · ' + batches.length + (batches.length === 1 ? ' batch' : ' batches') : ''}</p>
            <div class="downloads-overview-actions">
              <span class="overview-chip" class:idle={!activeJobCount}>{activeJobCount} in progress</span>
              <button class="button" type="button" onclick={clearHistory}
                disabled={clearingHistory || activeJobCount > 0 || (!jobs.length && !batches.length)}
                title={activeJobCount ? 'Finish or cancel queued downloads first' : 'Clear download history'}>
                Clear history
              </button>
            </div>
          </div>
          {#if retryJob}
            {#key retryJob.id}
              <div class="retry-panel">
                <strong>{retryJob.status === 'queued' ? 'Resume' : 'Retry'} {retryJob.item.title}</strong>
                <DownloadOptions backend={retryJob.requestedBackend} allowReplace={retryJob.replace} replace={retryJob.replace}
                  {settings} signedIn={settings.signIn || retryJob.accountRequired || retryJob.failureKind === 'authentication'} busy={downloadBusy}
                  label={retryJob.status === 'queued' ? 'Resume' : 'Retry download'}
                  onSubmit={retryDownload} onClose={() => retryJob = null} />
              </div>
            {/key}
          {/if}
          {#if batches.length}<h3 class="download-section-label">Batches</h3>{/if}
          <BatchView {batches} {jobs} {settings} {states} onChanged={refreshDownloads}
            onOpen={openRecordedItem} onRetry={(job) => retryJob = job} onCancel={cancelDownload}
            onFolder={(id) => jobAction(() => desktop.openDownloadFolder(id))}
            onRemove={(id) => jobAction(() => desktop.removeDownload(id))} />
          {#if standaloneJobs.length || !batches.length}
          {#if batches.length}<h3 class="download-section-label">Individual downloads</h3>{/if}
          <DownloadsView jobs={standaloneJobs} {states} games={settings.games} loading={jobsLoading} failed={jobsFailed}
            onOpen={openRecordedItem} onRetry={(job) => retryJob = job} onCancel={cancelDownload}
            onFolder={(id) => jobAction(() => desktop.openDownloadFolder(id))}
            onRemove={(id) => jobAction(() => desktop.removeDownload(id))} />
          {/if}
        </div>
      {/if}

      <div class="scroll-page" hidden={page !== 'settings'}>
        <SettingsView {settings} {states} {setup} onSetup={() => setup = true} onCloseSetup={() => setup = false} onSaved={updateSettings} onImported={() => { void refreshDownloads(); void refreshItemDownload(); void refreshBadges(); }} />
      </div>
    {/if}
  </main>
</div>
