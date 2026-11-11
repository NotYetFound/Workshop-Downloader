<script lang="ts">
  import { onMount } from 'svelte';
  import { desktop, errorMessage } from '../desktop';
  import type { DownloadActivity } from '../contracts/DownloadActivity';
  import type { TerminalSession } from '../contracts/TerminalSession';
  import type { DownloadJob } from '../contracts/DownloadJob';

  let tab = $state<'terminal' | 'logs'>('terminal');
  let sessions = $state<TerminalSession[]>([]);
  let selectedSession = $state<string | null>(null);
  const terminal = $derived(sessions.find((session) => session.id === selectedSession) ?? sessions[0]);
  let entries = $state<DownloadActivity[]>([]);
  let jobs = $state<DownloadJob[]>([]);
  let loading = $state(true);
  let error = $state('');
  let checking = false;
  const active = $derived(jobs.find((job) => ['preparing', 'authenticating', 'downloading', 'finalizing'].includes(job.status)));
  const queued = $derived(jobs.filter((job) => job.status === 'queued').length);

  async function refresh() {
    if (checking) return;
    checking = true;
    try {
      if (tab === 'terminal') sessions = await desktop.terminalSessions();
      else [entries, jobs] = await Promise.all([desktop.downloadActivity(), desktop.downloads()]);
      error = '';
    } catch (failure) {
      error = errorMessage(failure);
    } finally {
      checking = false;
      loading = false;
    }
  }

  onMount(() => {
    void refresh();
    const timer = window.setInterval(refresh, 750);
    return () => window.clearInterval(timer);
  });
</script>

<main class="download-logs-window">
  <header>
    <nav class="debug-tabs" aria-label="Debug views">
      <button type="button" class="button" aria-pressed={tab === 'terminal'} onclick={() => { tab = 'terminal'; void refresh(); }}>Terminal</button>
      <button type="button" class="button" aria-pressed={tab === 'logs'} onclick={() => { tab = 'logs'; void refresh(); }}>Logs</button>
    </nav>
    {#if tab === 'logs'}
    <p>{#if active}{active.item.title} · {active.status}{#if active.percent !== null} · {active.percent.toFixed(1)}%{/if}{:else}No active download{/if} · {queued} queued</p>
    {/if}
  </header>
  {#if error}<p class="error-text" role="alert">{error}</p>{/if}
  {#if tab === 'terminal'}
    {#if terminal}
      <nav class="debug-tabs terminal-tabs" aria-label="Download processes">
        {#each sessions as session (session.id)}
          <button type="button" class="button" aria-pressed={terminal.id === session.id}
            onclick={() => selectedSession = session.id}>
            {new Date(session.started).toLocaleTimeString()} · {session.title}
          </button>
        {/each}
      </nav>
      <p class="muted">{terminal.finished ? 'Ended ' + new Date(terminal.finished).toLocaleTimeString() : 'Running'} · Latest 250 lines</p>
      <!-- svelte-ignore a11y_no_noninteractive_tabindex (Keyboard users need to scroll the output.) -->
      <div class="live-download-log terminal-output" role="region" aria-label="Terminal output, newest first" tabindex="0">
        {#each [...terminal.lines].reverse() as line}
          <article><time datetime={line.time}>{new Date(line.time).toLocaleTimeString()}</time><pre>{line.text}</pre></article>
        {:else}<p>Waiting for output…</p>{/each}
      </div>
    {:else}
      <p class="muted">{loading ? 'Loading terminals…' : 'Terminal output appears when a download or engine check starts.'}</p>
    {/if}
  {:else}
  <!-- svelte-ignore a11y_no_noninteractive_tabindex (Keyboard users need to scroll the log.) -->
  <div class="live-download-log" aria-label="Download activity, newest first" tabindex="0" role="region">
    {#if loading}<p>Loading activity…</p>{/if}
    {#each entries as entry}
      <article>
        <time datetime={entry.time} title={new Date(entry.time).toLocaleString()}>{new Date(entry.time).toLocaleTimeString()}</time>
        <div><strong>{entry.title}</strong><p>{entry.message}</p></div>
      </article>
    {:else}
      {#if !loading && !error}<p>No download activity yet.</p>{/if}
    {/each}
  </div>
  {/if}
</main>
