<script lang="ts">
  import type { Snippet } from 'svelte';

  let { label, onclick, disabled = false, children }: {
    label: string;
    onclick: () => void;
    disabled?: boolean;
    children: Snippet;
  } = $props();
</script>

<button class="icon-button" type="button" title={label} aria-label={label} {onclick} {disabled}>
  {@render children()}
</button>
