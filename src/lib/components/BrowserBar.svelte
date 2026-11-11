<script lang="ts">
  import { ArrowLeft, ArrowRight, ChevronDown, Gamepad2, House, Link2, RotateCw, X } from '@lucide/svelte';
  import { onDestroy } from 'svelte';
  import IconButton from './IconButton.svelte';
  import { desktop, errorMessage, parseAppId } from '../desktop';
  import type { BrowserAction } from '../contracts/BrowserAction';
  import type { GamePreference } from '../contracts/GamePreference';

  let { games, appId, address = $bindable(), editing = $bindable(false), disabled, onAction, onAddress, onGame, onManage }: {
    games: GamePreference[];
    appId: number | null;
    address: string;
    editing?: boolean;
    disabled: boolean;
    onAction: (action: BrowserAction) => void;
    onAddress: (address: string) => void;
    onGame: (appId: number) => void;
    onManage: () => void;
  } = $props();

  let copyStatus = $state('');
  let copyTimer: ReturnType<typeof setTimeout>;
  onDestroy(() => clearTimeout(copyTimer));
  async function copyUrl() {
    try {
      await desktop.copyWorkshopUrl();
      copyStatus = 'Copied';
    } catch (error) {
      copyStatus = errorMessage(error);
    }
    clearTimeout(copyTimer);
    copyTimer = setTimeout(() => copyStatus = '', 2000);
  }
  let showAppId = $state(false);
  let appIdText = $state('');
  let appIdError = $state('');
  const savedGame = $derived(games.find((game) => game.appId === appId));

  function chooseGame(event: Event & { currentTarget: HTMLSelectElement }) {
    const { value } = event.currentTarget;
    event.currentTarget.value = '';
    if (value === 'manage') onManage();
    else if (value === 'app-id') showAppId = true;
    else {
      showAppId = false;
      onGame(Number(value));
    }
  }

  function openGame(event: SubmitEvent) {
    event.preventDefault();
    const id = parseAppId(appIdText);
    if (!id) {
      appIdError = 'Enter a valid Steam App ID.';
      return;
    }
    showAppId = false;
    appIdError = '';
    onGame(id);
  }
</script>

<div class="browser-toolbar" aria-label="Workshop navigation">
  <div class="history-buttons">
    <IconButton label="Back" onclick={() => onAction('back')} {disabled}><ArrowLeft size={16} /></IconButton>
    <IconButton label="Forward" onclick={() => onAction('forward')} {disabled}><ArrowRight size={16} /></IconButton>
    <IconButton label="Refresh" onclick={() => onAction('refresh')} {disabled}><RotateCw size={15} /></IconButton>
    <IconButton label="Workshop home" onclick={() => onAction('home')} {disabled}><House size={16} /></IconButton>
  </div>

  <form class="address-bar" onsubmit={(event) => {
    event.preventDefault();
    editing = false;
    onAddress(address);
  }}>
    <Link2 size={15} aria-hidden="true" />
    <input aria-label="Steam Workshop URL" placeholder="Enter Steam Workshop URL" spellcheck="false"
      autocomplete="off" bind:value={address} onfocus={() => editing = true} onblur={() => editing = false}
      oninput={() => editing = true} />
    <button type="button" class="address-go" title={copyStatus || 'Copy current URL'} disabled={disabled} onclick={copyUrl}>{copyStatus === 'Copied' ? 'Copied' : 'Copy'}</button>
  </form>

  <label class="game-selector">
    <Gamepad2 size={15} aria-hidden="true" />
    <select aria-label="Choose a game" onchange={chooseGame} {disabled}>
      <option value="" selected disabled>{savedGame?.name ?? (appId ? 'App ' + appId : 'Games')}</option>
      {#each games as game (game.appId)}<option value={game.appId}>{game.name}</option>{/each}
      <option value="app-id">Open by App ID…</option>
      <option value="manage">Manage saved games…</option>
    </select>
    <ChevronDown size={13} aria-hidden="true" />
  </label>
</div>

{#if showAppId}
  <form class="browser-subbar" onsubmit={openGame}>
    <label for="open-app-id">Steam App ID</label>
    <input id="open-app-id" inputmode="numeric" placeholder="e.g. 294100" bind:value={appIdText} required />
    <button class="button primary" type="submit" {disabled}>Open</button>
    {#if appIdError}<span class="error-text" role="alert">{appIdError}</span>{/if}
    <IconButton label="Close App ID field" onclick={() => showAppId = false}><X size={15} /></IconButton>
  </form>
{/if}
