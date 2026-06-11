<script lang="ts">
  import type { Locales } from '../i18n/i18n-types'
  import LL from '../i18n/i18n-svelte'
  import { highlightLkCode } from '../lib/highlight'
  import { parseDocMarkdown, buildDocNav } from '../lib/docPage'
  import stdlibDocument from '../stdlib/STDLIB.md?raw'
  import stdlibZhDocument from '../stdlib/STDLIB_zh.md?raw'

  export let locale: Locales = 'en'

  $: activeDoc = locale === 'zh-CN' ? stdlibZhDocument : stdlibDocument
  $: sections = parseDocMarkdown(activeDoc)
  $: nav = buildDocNav(sections)

  function highlightInlineCode(text: string): string {
    return text
      .replace(/&/g, '&amp;')
      .replace(/</g, '&lt;')
      .replace(/>/g, '&gt;')
      .replace(/`([^`]+)`/g, '<code class="inline-code">$1</code>')
  }
</script>

<section class="doc-layout" aria-label="LK standard library reference">
  <aside class="doc-toc" aria-label="Standard library table of contents">
    <strong>{$LL.stdlib.toc()}</strong>
    {#each nav as group}
      <details open>
        <summary>
          <span>{group.section.title}</span>
        </summary>
        {#each group.children as child}
          <a class="toc-nested" href={`#${child.id}`}>{child.title}</a>
        {/each}
      </details>
    {/each}
  </aside>

  <div class="doc-content">
    <div id="stdlib-start" class="doc-start-anchor" aria-hidden="true"></div>
    {#each sections as section, index}
      <article class={`doc-card doc-level-${section.level}`} id={section.id}>
        {#if section.level <= 2}
          <p class="doc-index">{String(index + 1).padStart(2, '0')}</p>
        {/if}
        {#if section.level === 2}
          <h2>{section.title}</h2>
        {:else if section.level === 3}
          <h3>{section.title}</h3>
        {:else}
          <h4>{section.title}</h4>
        {/if}

        {#each section.blocks as block}
          {#if block.type === 'paragraph'}
            <p>{@html highlightInlineCode(block.text)}</p>
          {:else if block.type === 'list'}
            <ul>
              {#each block.items as item}
                <li>{@html highlightInlineCode(item)}</li>
              {/each}
            </ul>
          {:else if block.type === 'code'}
            <pre><code>{@html highlightLkCode(block.text)}</code></pre>
          {:else if block.type === 'table'}
            <div class="doc-table-wrapper">
              <table>
                <thead>
                  <tr>
                    {#each block.headers as header}
                      <th>{header}</th>
                    {/each}
                  </tr>
                </thead>
                <tbody>
                  {#each block.rows as row}
                    <tr>
                      {#each row as cell, ci}
                        <td>{@html highlightInlineCode(cell)}</td>
                      {/each}
                    </tr>
                  {/each}
                </tbody>
              </table>
            </div>
          {/if}
        {/each}
      </article>
    {/each}
  </div>
</section>

<style>
  .doc-layout {
    display: grid;
    grid-template-columns: 16rem minmax(0, 1fr);
    gap: 2rem;
    max-width: 72rem;
    margin: 0 auto;
    padding: 6.5rem 1rem 4rem;
    scroll-margin-top: 6.5rem;
  }

  .doc-toc {
    position: sticky;
    top: 7rem;
    max-height: calc(100vh - 8rem);
    overflow-y: auto;
    font-size: 0.85rem;
    line-height: 1.6;
  }

  .doc-toc strong {
    display: block;
    margin-bottom: 0.75rem;
    font-size: 0.78rem;
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--muted);
  }

  .doc-toc details {
    margin-bottom: 0.5rem;
  }

  .doc-toc summary {
    cursor: pointer;
    font-weight: 600;
    color: var(--ink);
    padding: 0.25rem 0;
    list-style: none;
    display: flex;
    align-items: center;
    gap: 0.35rem;
  }

  .doc-toc summary::before {
    content: '▸';
    font-size: 0.7rem;
    color: var(--muted);
    transition: transform 0.15s;
  }

  .doc-toc details[open] summary::before {
    transform: rotate(90deg);
  }

  .doc-toc a {
    display: block;
    padding: 0.15rem 0 0.15rem 1rem;
    color: var(--ink-soft);
    text-decoration: none;
    font-size: 0.82rem;
    border-radius: 3px;
    transition: color 0.15s, background 0.15s;
  }

  .doc-toc a:hover {
    color: var(--accent);
    background: var(--accent-soft);
  }

  .doc-toc .toc-nested {
    padding-left: 1.5rem;
    font-size: 0.8rem;
  }

  .doc-content {
    min-width: 0;
  }

  .doc-start-anchor {
    height: 0;
  }

  .doc-card {
    padding: 1.5rem 0;
  }

  .doc-card h2 {
    font-size: 1.35rem;
    font-weight: 700;
    margin: 0 0 0.75rem;
    color: var(--ink);
    display: flex;
    align-items: baseline;
    gap: 0.75rem;
  }

  .doc-card h3 {
    font-size: 1.1rem;
    font-weight: 600;
    margin: 1.25rem 0 0.5rem;
    color: var(--ink);
  }

  .doc-card h4 {
    font-size: 0.95rem;
    font-weight: 600;
    margin: 1rem 0 0.4rem;
    color: var(--ink-soft);
  }

  .doc-card p {
    margin: 0.5rem 0;
    line-height: 1.65;
    color: var(--ink-soft);
  }

  .doc-index {
    font-size: 0.7rem;
    font-weight: 800;
    color: var(--accent);
    letter-spacing: 0.04em;
    margin: 0;
    line-height: 1;
  }

  .doc-card ul {
    margin: 0.5rem 0;
    padding-left: 1.25rem;
    color: var(--ink-soft);
    line-height: 1.65;
  }

  .doc-card li {
    margin: 0.2rem 0;
  }

  .doc-card pre {
    margin: 0.75rem 0;
    padding: 1rem;
    overflow-x: auto;
    border: 1px solid var(--code-border);
    border-radius: var(--radius);
    background: var(--code-bg);
    color: var(--code-text);
    font: 500 0.86rem/1.65 var(--font-mono);
  }

  .doc-table-wrapper {
    overflow-x: auto;
    margin: 0.75rem 0;
  }

  .doc-table-wrapper table {
    width: 100%;
    border-collapse: collapse;
    font-size: 0.85rem;
  }

  .doc-table-wrapper th {
    text-align: left;
    font-weight: 600;
    color: var(--ink);
    padding: 0.5rem 0.75rem;
    border-bottom: 2px solid var(--line);
    background: var(--surface-2);
    white-space: nowrap;
  }

  .doc-table-wrapper td {
    padding: 0.4rem 0.75rem;
    border-bottom: 1px solid var(--line-soft);
    color: var(--ink-soft);
    vertical-align: top;
  }

  .doc-table-wrapper td:first-child {
    font-family: var(--font-mono);
    font-size: 0.82rem;
    color: var(--accent-ink);
    white-space: nowrap;
  }

  @media (max-width: 860px) {
    .doc-layout {
      grid-template-columns: 1fr;
      padding-top: 5rem;
    }

    .doc-toc {
      position: static;
      max-height: none;
      border-bottom: 1px solid var(--line-soft);
      padding-bottom: 1rem;
      margin-bottom: 1rem;
    }
  }
</style>
