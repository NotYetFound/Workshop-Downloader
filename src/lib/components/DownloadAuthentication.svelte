<script lang="ts">
  import { desktop, errorMessage } from '../desktop';
  import type { AuthPrompt } from '../contracts/AuthPrompt';

  let { prompt, onCancel }: { prompt: AuthPrompt; onCancel: (id: string) => void } = $props();
  let response = $state('');
  let error = $state('');
  let busy = $state(false);
  const qr = $derived.by(() => {
    const rows = prompt.qr?.split('\n');
    if (!rows?.length || rows.some((row) => row.length !== rows.length || /[^01]/.test(row))) return null;
    const path = rows.flatMap((row, y) => [...row].flatMap((cell, x) => cell === '1' ? [`M${x} ${y}h1v1h-1z`] : [])).join('');
    return { size: rows.length, path };
  });

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    busy = true;
    const pending = desktop.authenticateDownload(prompt.jobId, response);
    response = '';
    try {
      await pending;
      error = '';
    } catch (failure) {
      error = errorMessage(failure);
    } finally {
      busy = false;
    }
  }
</script>

<form class="browser-subbar auth-strip" onsubmit={submit}>
  {#if prompt.kind === 'qr'}
    <div class="steam-qr">
      <strong>Scan with the Steam mobile app.</strong>
      {#if qr}
        <svg viewBox={`0 0 ${qr.size} ${qr.size}`} role="img" aria-label="Steam sign-in QR code" shape-rendering="crispEdges">
          <rect width={qr.size} height={qr.size} fill="#fff" />
          <path d={qr.path} fill="#000" />
        </svg>
      {:else}
        <span class="field-note" role="status">Waiting for the engine’s QR code…</span>
      {/if}
    </div>
  {:else if prompt.kind === 'mobile'}
    <strong>Approve this sign-in in the Steam mobile app.</strong>
  {:else}
    <label for="download-auth">{prompt.kind === 'account' ? 'Steam account name' : prompt.kind === 'password' ? 'Steam password' : 'Steam Guard code'}</label>
    <input id="download-auth" type={prompt.kind === 'account' ? 'text' : 'password'} autocomplete="off" spellcheck="false" bind:value={response} maxlength={prompt.kind === 'account' ? 64 : 1024} required disabled={busy} />
    <button class="button primary" type="submit" disabled={busy || !response}>Continue</button>
  {/if}
  <button class="button" type="button" onclick={() => onCancel(prompt.jobId)}>Cancel download</button>
  {#if error}<span class="error-text" role="alert">{error}</span>{/if}
</form>
