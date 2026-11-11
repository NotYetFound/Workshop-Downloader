<script lang="ts">
  import { onMount } from 'svelte';
  import DownloadAuthentication from './DownloadAuthentication.svelte';
  import { desktop, errorMessage } from '../desktop';
  import type { AuthPrompt } from '../contracts/AuthPrompt';

  let prompt = $state<AuthPrompt | null>(null);
  let error = $state('');
  let checking = false;

  async function refresh() {
    if (checking) return;
    checking = true;
    try {
      prompt = await desktop.downloadPrompt();
    } catch (failure) {
      error = errorMessage(failure);
    } finally {
      checking = false;
    }
  }

  async function cancel(id: string) {
    try {
      await desktop.cancelDownload(id);
    } catch (failure) {
      error = errorMessage(failure);
    }
  }

  onMount(() => {
    void refresh();
    const timer = window.setInterval(refresh, 300);
    return () => window.clearInterval(timer);
  });
</script>

<main class="download-auth-window">
  <h1>Steam sign-in</h1>
  {#if prompt}
    {#key prompt.jobId + ':' + prompt.kind}
      <DownloadAuthentication {prompt} onCancel={cancel} />
    {/key}
  {:else}
    <p role="status">Waiting for the download engine…</p>
  {/if}
  {#if error}<p class="error-text" role="alert">{error}</p>{/if}
</main>
