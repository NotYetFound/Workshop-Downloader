<script lang="ts">
  import { FolderOpen, Plus, Trash2 } from '@lucide/svelte';
  import { untrack } from 'svelte';
  import { desktop, errorMessage, parseAppId } from '../desktop';
  import type { Settings } from '../contracts/Settings';
  import type { FolderNaming } from '../contracts/FolderNaming';
  import IconButton from './IconButton.svelte';
  import Panel from './Panel.svelte';
  import ExternalDownloads from './ExternalDownloads.svelte';
  import type { ItemDownloadState } from '../contracts/ItemDownloadState';
  import DownloadBackends from './DownloadBackends.svelte';

  let { settings, onSaved, states, onImported }: { settings: Settings; onSaved: (value: Settings) => void; states: Record<string, ItemDownloadState>; onImported: () => void } = $props();

  function copy(value: Settings): Settings {
    return { ...value, games: value.games.map((game) => ({ ...game })) };
  }

  let draft = $state(copy(untrack(() => settings)));
  let saving = $state(false);
  let choosing = $state(false);
  let message = $state('');
  let failure = $state('');
  let newId = $state('');
  let newName = $state('');
  const dirty = $derived(JSON.stringify(draft) !== JSON.stringify(settings));
  const namingOptions: { value: FolderNaming; label: string; example: string }[] = [
    { value: 'title', label: 'Title', example: 'Coastal Town' },
    { value: 'titleAndId', label: 'Title + ID', example: 'Coastal Town (12345)' },
    { value: 'id', label: 'Workshop ID', example: '12345' },
  ];

  async function chooseFolder(appId?: number) {
    const game = draft.games.find((game) => game.appId === appId);
    choosing = true;
    failure = '';
    try {
      const selected = await desktop.selectFolder(game?.downloadDir ?? draft.downloadDir);
      if (selected === null) return;
      if (game) game.downloadDir = selected;
      else draft.downloadDir = selected;
    } catch (error) {
      failure = errorMessage(error);
    } finally {
      choosing = false;
    }
  }

  function addGame(event: SubmitEvent) {
    event.preventDefault();
    const appId = parseAppId(newId);
    if (!appId || draft.games.some((game) => game.appId === appId) || !newName.trim()) {
      failure = 'Enter a game name and a Steam App ID that is not already saved.';
      return;
    }
    draft.games.push({ appId, name: newName.trim(), downloadDir: null });
    newId = '';
    newName = '';
    message = '';
    failure = '';
  }

  async function save() {
    saving = true;
    failure = '';
    message = '';
    const next = copy(draft);
    try {
      await desktop.saveSettings(next);
      onSaved(next);
      message = 'Preferences saved.';
    } catch (error) {
      failure = errorMessage(error);
    } finally {
      saving = false;
    }
  }
</script>

<div class="settings-stack">
  <Panel className="settings-primary" title="Download destination">
    <label class="field-label" for="download-directory">Download directory</label>
    <div class="field-row">
      <input id="download-directory" bind:value={draft.downloadDir} spellcheck="false" disabled={saving} />
      <button class="button secondary" type="button" onclick={() => chooseFolder()} disabled={saving || choosing}><FolderOpen size={16} /> Browse</button>
    </div>
    <label class="toggle-row">
      <span><strong>Organize by game</strong><small>Keep each game’s downloads in its own folder.</small></span>
      <input class="switch" type="checkbox" bind:checked={draft.organizeByGame} disabled={saving} />
    </label>
  </Panel>

  <Panel className="settings-primary settings-naming" title="Folder names">
    <fieldset class="naming-options" aria-label="Mod folder naming">
      {#each namingOptions as option (option.value)}
        <label class="radio-card">
          <input type="radio" name="folder-naming" bind:group={draft.folderNaming} value={option.value} disabled={saving} />
          <span><strong>{option.label}</strong><small>{option.example}</small></span>
        </label>
      {/each}
    </fieldset>
    <p class="field-note">Duplicate folder names include the Workshop ID.</p>
  </Panel>

  <Panel title="Saved games">
    <div class="preference-games">
      <div class="preference-game game-columns" aria-hidden="true">
        <span>Game name / App ID</span><span>Download override</span><span></span><span></span>
      </div>
      {#each draft.games as game (game.appId)}
        <div class="preference-game">
          <div class="game-field">
            <input aria-label={'Name for App ' + game.appId} bind:value={game.name} maxlength={120} disabled={saving} />
            <code>{game.appId}</code>
          </div>
          <input
            aria-label={'Download override for ' + game.name}
            placeholder="Use default destination"
            value={game.downloadDir ?? ''}
            oninput={(event) => game.downloadDir = event.currentTarget.value || null}
            spellcheck="false"
            disabled={saving}
          />
          <IconButton label={'Choose folder for ' + game.name} onclick={() => chooseFolder(game.appId)} disabled={saving || choosing}><FolderOpen size={17} /></IconButton>
          <IconButton label={'Remove ' + game.name} onclick={() => draft.games = draft.games.filter((entry) => entry.appId !== game.appId)} disabled={saving}><Trash2 size={16} /></IconButton>
        </div>
      {/each}
    </div>
    <form class="add-game" onsubmit={addGame}>
      <label><span class="field-label">Game name</span><input placeholder="e.g. Stellaris" bind:value={newName} maxlength={120} required disabled={saving} /></label>
      <label><span class="field-label">Steam App ID</span><input placeholder="281990" inputmode="numeric" bind:value={newId} required disabled={saving} /></label>
      <button class="button secondary" type="submit" disabled={saving}><Plus size={16} /> Add game</button>
    </form>
    <p class="field-note">An override is the game’s root. Leave it blank to use the default.</p>
  </Panel>

  <DownloadBackends bind:settings={draft} disabled={saving} />

  <ExternalDownloads {states} {onImported} />

  {#if dirty || saving || failure || message}
  <div class="settings-actions">
    <div aria-live="polite">
      {#if failure}<span class="error-text" role="alert">{failure}</span>
      {:else if dirty}<span class="muted">Unsaved changes</span>
      {:else if message}<span class="success-text">{message}</span>
      {/if}
    </div>
    <button class="button secondary" type="button" disabled={!dirty || saving} onclick={() => {
      draft = copy(settings);
      failure = '';
      message = '';
    }}>Discard</button>
    <button class="button primary" type="button" disabled={!dirty || saving || choosing} onclick={save}>{saving ? 'Saving…' : 'Save preferences'}</button>
  </div>
  {/if}
</div>
