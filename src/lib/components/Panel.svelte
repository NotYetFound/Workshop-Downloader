<script lang="ts">
  import type { Snippet } from 'svelte';
  let { title, description, className = '', children }: { title: string; description?: string; className?: string; children: Snippet } = $props();
</script>

<section class="panel {className}">
  <div class="panel-heading"><h2>{title}</h2>{#if description}<p>{description}</p>{/if}</div>
  <div class="panel-body">{@render children()}</div>
</section>
