<script lang="ts">
  import { onMount } from 'svelte';
  import { confirm } from '@tauri-apps/plugin-dialog';
  import { ExternalLink, FolderOpen } from '@lucide/svelte';
  import { backendNames, desktop, errorMessage } from '../desktop';
  import type { BackendChoice } from '../contracts/BackendChoice';
  import type { BackendInfo } from '../contracts/BackendInfo';
  import type { Settings } from '../contracts/Settings';
  import IconButton from './IconButton.svelte';
  import Panel from './Panel.svelte';

  let { settings = $bindable(), disabled, setup = false }: { settings: Settings; disabled: boolean; setup?: boolean } = $props();
  let info = $state<BackendInfo[]>([]);
  let checking = $state(false);
  let error = $state('');
  let message = $state('');
  let working = $state<BackendChoice | null>(null);
  let issues = $state<Record<string, string>>({});
  type ExecutableField = 'steamcmdPath' | 'depotDownloaderPath';
  let locationPicker = $state<{ key: ExecutableField; paths: string[] } | null>(null);
  let choosing = $state(false);
  let revision = 0;
  const backends = [
    { backend: 'steamcmd', key: 'steamcmdPath' },
    { backend: 'depotDownloader', key: 'depotDownloaderPath' },
  ] as const;

  async function refreshIssues() {
    issues = await desktop.backendIssues();
  }

  onMount(() => {
    const poll = () => void refreshIssues().catch(() => {});
    poll();
    const timer = window.setInterval(poll, 2000);
    return () => window.clearInterval(timer);
  });

  async function clearIssue(backend: BackendChoice) {
    try {
      await desktop.clearBackendIssue(backend);
      await refreshIssues();
    } catch (failure) {
      error = errorMessage(failure);
    }
  }

  $effect(() => {
    const snapshot = $state.snapshot(settings);
    const timer = window.setTimeout(() => void update(snapshot, false), 220);
    return () => window.clearTimeout(timer);
  });

  async function update(snapshot: Settings, check: boolean) {
    if (!check && checking) return;
    const sequence = ++revision;
    if (check) checking = true;
    try {
      const next = await desktop.downloadBackends(snapshot, check);
      if (sequence === revision) {
        info = next;
        error = '';
      }
    } catch (failure) {
      if (sequence === revision && check) error = errorMessage(failure);
    } finally {
      if (check) checking = false;
    }
  }

  async function manage(backend: BackendChoice, action: 'install' | 'restore' | 'clear') {
    working = backend;
    error = '';
    message = '';
    try {
      if (action === 'install') {
        const release = await desktop.backendRelease(backend);
        if (!release.updateAvailable && backend === 'depotDownloader'
          && info.find((engine) => engine.backend === backend)?.state !== 'unavailable') {
          message = `${backendNames[backend]} ${release.version} is up to date.`;
          return;
        }
        const integrity = release.checksumAvailable
          ? 'The publisher’s SHA-256 checksum will be verified.'
          : 'The publisher provides this archive over HTTPS without a published checksum.';
        if (!await confirm(`${backendNames[backend]} · ${release.version}\n\n${release.source}\n\n${integrity}`,
          { title: 'Install official download engine', okLabel: 'Install' })) return;
        await desktop.installBackend(backend, release.source);
        await update($state.snapshot(settings), true);
        message = `${backendNames[backend]} installed.`;
      } else if (action === 'restore') {
        await desktop.restoreBackend(backend);
        await update($state.snapshot(settings), true);
        message = `Previous ${backendNames[backend]} version restored.`;
      } else {
        if (!await confirm(`Clear local ${backendNames[backend]} sign-in data? You may need to sign in again.`,
          { title: 'Clear Steam session', kind: 'warning', okLabel: 'Clear session' })) return;
        await desktop.clearBackendSession(backend);
        message = `${backendNames[backend]} session cleared.`;
      }
    } catch (failure) {
      error = errorMessage(failure);
    } finally {
      working = null;
    }
  }

  async function browse(key: ExecutableField) {
    if (locationPicker?.key === key) {
      locationPicker = null;
      return;
    }
    choosing = true;
    const paths = await desktop.executableLocations().catch(() => []);
    choosing = false;
    locationPicker = paths.length ? { key, paths } : null;
    if (!paths.length) await choose(key);
  }

  async function choose(key: ExecutableField, location?: string) {
    choosing = true;
    error = '';
    try {
      const path = await desktop.selectExecutable(location ?? settings[key] ?? undefined);
      if (path) {
        settings[key] = path;
        locationPicker = null;
      }
    } catch (failure) {
      error = errorMessage(failure);
    } finally {
      choosing = false;
    }
  }
</script>

<Panel title="Download engines" description="Set your default engine and Steam sign-in preferences.">
  {#if setup}
    <p>New to Workshop Downloader? Use <strong>Set up</strong> for DepotDownloader, then keep <strong>Automatic</strong> as the default.</p>
    <p class="field-note">Already have an engine? Browse to its executable and use Check backends to verify it. For SteamCMD on Linux, choose Valve’s extracted steamcmd.sh alongside its linux32 folder.</p>
  {/if}
  <div class="backend-default">
    <label for="default-downloader">Default backend</label>
    <select id="default-downloader" bind:value={settings.backend} {disabled}>
      {#each Object.entries(backendNames) as [id, name]}<option value={id}>{name}</option>{/each}
    </select>
    <button type="button" class="button" onclick={() => update($state.snapshot(settings), true)} disabled={checking || disabled || working !== null}>
      {checking ? 'Checking…' : 'Check backends'}
    </button>
  </div>
  {#if !setup}
  <div class="backend-default">
    <label class="backend-sign-in"><span>Sign in with Steam by default</span><input class="switch" type="checkbox" role="switch" bind:checked={settings.signIn} {disabled} /></label>
    <input aria-label="Steam account name" placeholder="Steam account name" bind:value={settings.steamAccount} autocomplete="username" spellcheck="false" maxlength={64} {disabled} />
  </div>
  <label class="toggle-row backend-sign-in">
    <span>Use Steam mobile QR when signing in with DepotDownloader</span>
    <input class="switch" type="checkbox" role="switch" bind:checked={settings.depotQr} {disabled} />
  </label>
  {/if}
  <p class="field-note">Public items can download anonymously. If an engine needs sign-in, it opens a separate window.</p>
  {#each backends as { backend, key }}
    {@const detected = info.find((candidate) => candidate.backend === backend)}
    <div class="backend-line">
      <div class="backend-heading">
        <label for={'executable-' + backend}>{backendNames[backend]}</label>
        {#if detected}
          <span class:success-text={detected.state === 'ready'} class="muted">
            {detected.state === 'missing' ? 'Missing' : detected.state === 'detected' ? 'Detected' : detected.state === 'ready' ? 'Operational' : 'Unavailable'}
            {detected.version ? ' · ' + detected.version : ''}{detected.managed ? ' · Managed' : ''}
          </span>
        {/if}
      </div>
      <input id={'executable-' + backend} placeholder={detected?.path ?? 'Detect automatically'} title={detected?.path}
        value={settings[key] ?? ''} oninput={(event) => settings[key] = event.currentTarget.value || null}
        spellcheck="false" disabled={disabled || checking || working !== null} />
      <IconButton label={'Browse for ' + backendNames[backend]} onclick={() => browse(key)} disabled={disabled || checking || choosing || working !== null}><FolderOpen size={15} /></IconButton>
      <IconButton label={'Open official ' + backendNames[backend] + ' setup page'} onclick={() => {
        void desktop.openBackendSource(backend).catch((failure) => error = errorMessage(failure));
      }}><ExternalLink size={15} /></IconButton>
      {#if locationPicker?.key === key}
        <div class="backend-note executable-locations" role="group" aria-label={'Browse locations for ' + backendNames[backend]}>
          <p class="field-note">Mounted drives</p>
          <div class="backend-actions">
            {#each locationPicker.paths as path (path)}
              <button class="button" type="button" title={path} disabled={choosing || disabled} onclick={() => choose(key, path)}>{path}</button>
            {/each}
            <button class="button" type="button" disabled={choosing || disabled} onclick={() => choose(key)}>Browse other locations…</button>
          </div>
        </div>
      {/if}
      <div class="backend-note backend-actions">
        <button class="button" type="button" disabled={disabled || checking || working !== null} onclick={() => manage(backend, 'install')}>
          {working === backend ? 'Working…' : detected?.managed ? backend === 'steamcmd' ? 'Refresh bootstrap' : 'Check for update' : 'Set up'}
        </button>
        {#if detected?.canRestore}
          <button class="button" type="button" disabled={disabled || checking || working !== null} onclick={() => manage(backend, 'restore')}>Restore previous</button>
        {/if}
        {#if !setup}
          <button class="button" type="button" disabled={disabled || checking || working !== null} onclick={() => manage(backend, 'clear')}>Clear session</button>
        {/if}
      </div>
      {#if detected?.message}<p class="field-note backend-note">{detected.message}</p>{/if}
      {#if issues[backend]}
        <div class="backend-note backend-actions">
          <p class="field-note error-text">{issues[backend]}</p>
          <button type="button" class="button" onclick={() => clearIssue(backend)}>Clear {backendNames[backend]} issue</button>
        </div>
      {/if}
    </div>
  {/each}
  <p class="field-note">One engine is enough. You can also choose a manually installed executable.</p>
  {#if message}<p class="field-note success-text" role="status">{message}</p>{/if}
  {#if error}<p class="field-note error-text" role="alert">{error}</p>{/if}
</Panel>
