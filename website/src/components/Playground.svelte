<script lang="ts">
  import { onMount } from 'svelte'
  import { Copy, Play, RotateCcw } from '@lucide/svelte'
  import './Playground.css'
  import LL from '../i18n/i18n-svelte'
  import LkCodeEditor from './LkCodeEditor.svelte'
  import { loadLkWasm, type RunResult } from '../lib/lkWasm'
  import { playgroundExamples } from '../lib/playgroundExamples'
  import type { PlaygroundSelectionId } from '../lib/playgroundExamples'

  const customExampleId: PlaygroundSelectionId = 'custom'

  export let embedded = false

  export let source = playgroundExamples[0].code
  export let activeExample: PlaygroundSelectionId = playgroundExamples[0].id
  let wasmReady = false
  let loading = true
  let running = false
  let loadError = ''
  let result: RunResult | undefined = undefined

  onMount(async () => {
    try {
      await loadLkWasm()
      wasmReady = true
    } catch (error) {
      loadError = error instanceof Error ? error.message : String(error)
    } finally {
      loading = false
    }
  })

  async function run(): Promise<void> {
    running = true
    result = undefined
    try {
      const wasm = await loadLkWasm()
      result = wasm.runLk(source)
    } catch (error) {
      result = {
        ok: false,
        stdout: '',
        result: null,
        error: error instanceof Error ? error.message : String(error),
        diagnostics: [],
        elapsedMs: 0,
      }
    } finally {
      running = false
    }
  }

  function reset(): void {
    const current = playgroundExamples.find((example) => example.id === activeExample) || playgroundExamples[0]
    source = current.code
    result = undefined
  }

  function selectExample(event: Event): void {
    const id = (event.currentTarget as HTMLSelectElement).value
    const example = playgroundExamples.find((item) => item.id === id)
    if (!example) return
    activeExample = example.id
    source = example.code
    result = undefined
  }

  async function copyOutput(): Promise<void> {
    const text = [result?.stdout, result?.result, result?.error].filter(Boolean).join('\n')
    if (!text) return

    try {
      if (navigator.clipboard) {
        await navigator.clipboard.writeText(text)
        return
      }
    } catch (error) {
      console.warn('Clipboard API copy failed; falling back to document copy.', error)
    }

    const textarea = document.createElement('textarea')
    textarea.value = text
    textarea.setAttribute('readonly', '')
    textarea.style.position = 'fixed'
    textarea.style.left = '-9999px'
    textarea.style.top = '0'
    document.body.appendChild(textarea)
    textarea.select()
    try {
      document.execCommand('copy')
    } catch (error) {
      console.warn('Fallback clipboard copy failed.', error)
    } finally {
      document.body.removeChild(textarea)
    }
  }
</script>

<section id="examples" class="playground-page" class:playground-embedded={embedded} aria-label={$LL.playground.ariaLabel()}>
  <div class="playground-shell">
    <section class="playground-editor" aria-label={$LL.playground.editorAriaLabel()}>
      <div class="playground-toolbar">
        <div class="window-controls" aria-hidden="true">
          <span class="dot red"></span>
          <span class="dot yellow"></span>
          <span class="dot green"></span>
        </div>
        <div class="editor-select-wrapper">
          <select value={activeExample} on:change={selectExample} aria-label={$LL.playground.selectSample()}>
            {#each playgroundExamples as example}
              <option value={example.id}>{$LL.playground.examples[example.id]()}</option>
            {/each}
            {#if activeExample === customExampleId}
              <option value={customExampleId}>{$LL.playground.examples.custom()}</option>
            {/if}
          </select>
        </div>
        <div class="playground-actions">
          <button class="icon-btn" type="button" on:click={reset} aria-label={$LL.playground.resetSource()}>
            <RotateCcw size={17} />
          </button>
          <button class="btn btn-primary" type="button" on:click={run} disabled={!wasmReady || running}>
            <Play size={18} />
            {running ? $LL.playground.running() : $LL.playground.run()}
          </button>
        </div>
      </div>
      <LkCodeEditor bind:value={source} ariaLabel={$LL.playground.sourceAriaLabel()} compact={embedded} />
    </section>

    <section class="playground-output" aria-label={$LL.playground.outputAriaLabel()}>
      <div class="output-header">
        <span>{loading ? $LL.playground.loadingWasm() : wasmReady ? $LL.playground.ready() : $LL.playground.unavailable()}</span>
        <button class="icon-btn" type="button" on:click={copyOutput} disabled={!result} aria-label={$LL.playground.copyOutput()}>
          <Copy size={17} />
        </button>
      </div>

      {#if loadError}
        <pre class="output-error">{loadError}</pre>
      {:else if loading}
        <div class="output-skeleton"></div>
        <div class="output-skeleton short"></div>
      {:else if result}
        <div class:output-ok={result.ok} class:output-fail={!result.ok} class="output-status">
          {result.ok ? $LL.playground.completed() : $LL.playground.failed()} · {result.elapsedMs.toFixed(1)} ms
        </div>
        {#if result.stdout}
          <h2>stdout</h2>
          <pre>{result.stdout}</pre>
        {/if}
        {#if result.result}
          <h2>result</h2>
          <pre>{result.result}</pre>
        {/if}
        {#if result.error}
          <h2>error</h2>
          <pre class="output-error">{result.error}</pre>
        {/if}
        {#each result.diagnostics as diagnostic}
          {#if diagnostic.rendered}
            <h2>{diagnostic.level}</h2>
            <pre class="output-error">{diagnostic.rendered}</pre>
          {/if}
        {/each}
      {:else}
        <div class="output-empty">
          <span>{$LL.playground.ready()}</span>
          <p>{$LL.playground.emptyMessage()}</p>
        </div>
      {/if}
    </section>
  </div>
</section>
