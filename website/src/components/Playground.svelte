<script lang="ts">
  import { onMount } from 'svelte'
  import { Copy, Play, RotateCcw } from '@lucide/svelte'
  import './Playground.css'
  import LkCodeEditor from './LkCodeEditor.svelte'
  import { loadLkWasm, type RunResult } from '../lib/lkWasm'
  import { playgroundExamples } from '../lib/playgroundExamples'

  let source = playgroundExamples[0].code
  let activeExample = playgroundExamples[0].name
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
    const current = playgroundExamples.find((example) => example.name === activeExample) || playgroundExamples[0]
    source = current.code
    result = undefined
  }

  function selectExample(event: Event): void {
    const name = (event.currentTarget as HTMLSelectElement).value
    const example = playgroundExamples.find((item) => item.name === name)
    if (!example) return
    activeExample = example.name
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

<section class="playground-page" aria-labelledby="try-title">
  <div class="playground-heading">
    <p id="try-title" class="eyebrow">LK Wasm Playground</p>
    <p>
      The playground loads the LK wasm runtime only on this page and runs a browser-safe stdlib subset.
    </p>
  </div>

  <div class="playground-shell">
    <section class="playground-editor" aria-label="LK source editor">
      <div class="playground-toolbar">
        <label>
          <span>Example</span>
          <select value={activeExample} on:change={selectExample}>
            {#each playgroundExamples as example}
              <option value={example.name}>{example.name}</option>
            {/each}
          </select>
        </label>
        <div class="playground-actions">
          <button class="icon-btn" type="button" on:click={reset} aria-label="Reset source">
            <RotateCcw size={17} />
          </button>
          <button class="btn btn-primary" type="button" on:click={run} disabled={!wasmReady || running}>
            <Play size={18} />
            {running ? 'Running' : 'Run'}
          </button>
        </div>
      </div>
      <LkCodeEditor bind:value={source} ariaLabel="LK source code" />
    </section>

    <section class="playground-output" aria-label="Run output">
      <div class="output-header">
        <span>{loading ? 'Loading wasm' : wasmReady ? 'Ready' : 'Unavailable'}</span>
        <button class="icon-btn" type="button" on:click={copyOutput} disabled={!result} aria-label="Copy output">
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
          {result.ok ? 'Completed' : 'Failed'} · {result.elapsedMs.toFixed(1)} ms
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
          <span>Ready</span>
          <p>Select an example or edit the source, then run it in the wasm sandbox.</p>
        </div>
      {/if}
    </section>
  </div>
</section>
