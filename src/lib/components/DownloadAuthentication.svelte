<script lang="ts">
  import { desktop, errorMessage } from '../desktop';
  import type { AuthPrompt } from '../contracts/AuthPrompt';

  let { prompt, onCancel }: { prompt: AuthPrompt; onCancel: (id: string) => void } = $props();
  let response = $state('');
  let error = $state('');
  let busy = $state(false);

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
  {#if prompt.kind === 'mobile'}
    <strong>Approve this sign-in in the Steam mobile app.</strong>
  {:else}
    <label for="download-auth">{prompt.kind === 'password' ? 'Steam password' : 'Steam Guard code'}</label>
    <input id="download-auth" type="password" autocomplete="off" bind:value={response} maxlength={1024} required disabled={busy} />
    <button class="button primary" type="submit" disabled={busy || !response}>Continue</button>
  {/if}
  <button class="button" type="button" onclick={() => onCancel(prompt.jobId)}>Cancel download</button>
  {#if error}<span class="error-text" role="alert">{error}</span>{/if}
</form>
