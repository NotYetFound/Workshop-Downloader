<script lang="ts">
  import { ExternalLink, FolderOpen } from '@lucide/svelte';
  import { backendNames, desktop, errorMessage } from '../desktop';
  import type { BackendInfo } from '../contracts/BackendInfo';
  import type { Settings } from '../contracts/Settings';
  import IconButton from './IconButton.svelte';
  import Panel from './Panel.svelte';

  let { settings = $bindable(), disabled }: { settings: Settings; disabled: boolean } = $props();
  let info = $state<BackendInfo[]>([]);
  let checking = $state(false);
  let error = $state('');
  let revision = 0;
  const backends = [
    { backend: 'steamcmd', key: 'steamcmdPath' },
    { backend: 'depotDownloader', key: 'depotDownloaderPath' },
  ] as const;

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

  async function choose(key: 'steamcmdPath' | 'depotDownloaderPath') {
    try {
      const path = await desktop.selectExecutable();
      if (path) settings[key] = path;
    } catch (failure) {
      error = errorMessage(failure);
    }
  }
</script>

<Panel title="Download engines" description="Set your default engine and Steam sign-in preferences.">
  <div class="backend-default">
    <label for="default-downloader">Default backend</label>
    <select id="default-downloader" bind:value={settings.backend} {disabled}>
      {#each Object.entries(backendNames) as [id, name]}<option value={id}>{name}</option>{/each}
    </select>
    <button type="button" class="button" onclick={() => update($state.snapshot(settings), true)} disabled={checking || disabled}>
      {checking ? 'Checking…' : 'Check backends'}
    </button>
  </div>
  <div class="backend-default">
    <label><input type="checkbox" bind:checked={settings.signIn} {disabled} /> Sign in with Steam by default</label>
    <input aria-label="Steam account name" placeholder="Steam account name" bind:value={settings.steamAccount} autocomplete="username" spellcheck="false" maxlength={64} {disabled} />
  </div>
  <p class="field-note">Passwords and Steam Guard codes are requested here when needed.</p>
  {#each backends as { backend, key }}
    {@const detected = info.find((candidate) => candidate.backend === backend)}
    <div class="backend-line">
      <div class="backend-heading">
        <label for={'executable-' + backend}>{backendNames[backend]}</label>
        {#if detected}
          <span class:success-text={detected.state === 'ready'} class="muted">
            {detected.state === 'missing' ? 'Missing' : detected.state === 'detected' ? 'Detected' : detected.state === 'ready' ? 'Operational' : 'Unavailable'}
            {detected.version ? ' · ' + detected.version : ''}
          </span>
        {/if}
      </div>
      <input id={'executable-' + backend} placeholder={detected?.path ?? 'Detect automatically'} title={detected?.path}
        value={settings[key] ?? ''} oninput={(event) => settings[key] = event.currentTarget.value || null}
        spellcheck="false" disabled={disabled || checking} />
      <IconButton label={'Browse for ' + backendNames[backend]} onclick={() => choose(key)} disabled={disabled || checking}><FolderOpen size={15} /></IconButton>
      <IconButton label={'Open official ' + backendNames[backend] + ' setup page'} onclick={() => {
        void desktop.openBackendSource(backend).catch((failure) => error = errorMessage(failure));
      }}><ExternalLink size={15} /></IconButton>
      {#if detected?.message}<p class="field-note backend-note">{detected.message}</p>{/if}
    </div>
  {/each}
  <p class="field-note">Use the official setup links, extract the release, then browse to its executable.</p>
  {#if error}<p class="field-note error-text" role="alert">{error}</p>{/if}
</Panel>
