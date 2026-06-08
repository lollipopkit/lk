<script lang="ts">
  import { highlightLkCode } from '../lib/highlight'

  export let value = ''
  export let ariaLabel = 'LK source code'

  let highlightLayer: HTMLPreElement

  function syncScroll(event: Event): void {
    const textarea = event.currentTarget as HTMLTextAreaElement
    if (!highlightLayer) return
    highlightLayer.scrollTop = textarea.scrollTop
    highlightLayer.scrollLeft = textarea.scrollLeft
  }
</script>

<div class="lk-code-editor">
  <pre class="lk-code-editor-highlight" aria-hidden="true" bind:this={highlightLayer}><code>{@html highlightLkCode(value)}</code>{#if value.endsWith('\n')}<br />{/if}</pre>
  <textarea bind:value spellcheck="false" aria-label={ariaLabel} on:scroll={syncScroll}></textarea>
</div>

<style>
  .lk-code-editor {
    position: relative;
    min-height: 28rem;
    background: var(--code-bg-strong);
  }

  .lk-code-editor-highlight,
  .lk-code-editor textarea {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    min-height: 28rem;
    margin: 0;
    border: 0;
    font: 500 0.91rem/1.7 var(--font-mono);
    letter-spacing: 0;
    padding: 1.25rem;
    tab-size: 2;
    white-space: pre;
    overflow: auto;
  }

  .lk-code-editor-highlight {
    pointer-events: none;
    background: var(--code-bg-strong);
    color: var(--code-text);
  }

  .lk-code-editor-highlight code {
    display: block;
    min-width: 100%;
  }

  .lk-code-editor textarea {
    z-index: 1;
    resize: none;
    outline: 0;
    background: transparent;
    color: transparent;
    caret-color: var(--code-text);
  }

  .lk-code-editor textarea::selection {
    background: rgba(145, 124, 86, 0.34);
    color: transparent;
  }

  @media (max-width: 560px) {
    .lk-code-editor,
    .lk-code-editor-highlight,
    .lk-code-editor textarea {
      min-height: 24rem;
      font-size: 0.8rem;
    }
  }
</style>
