<script lang="ts">
  import { onMount } from 'svelte'
  import { slide, fade } from 'svelte/transition'
  import { ArrowUp } from '@lucide/svelte'
  import type { Locales } from '../i18n/i18n-types'
  import LL from '../i18n/i18n-svelte'
  import { highlightLkCode } from '../lib/highlight'
  import { parseDocMarkdown, buildDocNav } from '../lib/docPage'
  import Playground from './Playground.svelte'
  import learnDocument from '../learn/LEARN.md?raw'
  import learnZhDocument from '../learn/LEARN_zh.md?raw'

  export let locale: Locales = 'en'

  $: activeDoc = locale === 'zh-CN' ? learnZhDocument : learnDocument
  $: sections = parseDocMarkdown(activeDoc)
  $: nav = buildDocNav(sections)
  let collapsedGroups = new Set<string>()
  let activeSectionId = ''
  let showBackToTop = false

  onMount(() => {
    const observer = new IntersectionObserver((entries) => {
      const visible = entries.filter(e => e.isIntersecting)
      if (visible.length > 0) {
        visible.sort((a, b) => a.boundingClientRect.top - b.boundingClientRect.top)
        activeSectionId = visible[0].target.id
      }
    }, {
      rootMargin: '-10% 0px -65% 0px'
    })

    const elements = document.querySelectorAll('.doc-card')
    elements.forEach((el) => observer.observe(el))

    const handleScroll = () => {
      showBackToTop = window.scrollY > 400
    }
    window.addEventListener('scroll', handleScroll)

    return () => {
      observer.disconnect()
      window.removeEventListener('scroll', handleScroll)
    }
  })

  const playgroundSections = new Set([
    'hello-lk', 'values--types', 'variables--scope', 'operators--expressions',
    'collections', 'control-flow', 'pattern-matching', 'functions--closures',
    'structs--traits',
  ])

  function shouldShowPlayground(sectionId: string): boolean {
    return playgroundSections.has(sectionId)
  }

  function isGroupOpen(groupId: string): boolean {
    return !collapsedGroups.has(groupId)
  }

  function toggleGroup(groupId: string): void {
    collapsedGroups = new Set(collapsedGroups)
    if (collapsedGroups.has(groupId)) {
      collapsedGroups.delete(groupId)
    } else {
      collapsedGroups.add(groupId)
    }
  }

  function highlightInlineCode(text: string): string {
    return text
      .replace(/&/g, '&amp;')
      .replace(/</g, '&lt;')
      .replace(/>/g, '&gt;')
      .replace(/`([^`]+)`/g, '<code class="inline-code">$1</code>')
  }
</script>

<section class="doc-layout" aria-label="LK tutorial">
  <aside class="doc-toc" aria-label="Tutorial table of contents">
    <strong>{$LL.learn.toc()}</strong>
    {#each nav as group}
      <div class="toc-group">
        <div class="toc-heading">
          <a class="toc-title" class:toc-active={activeSectionId === group.section.id} href={`#${group.section.id}`}>{group.section.title}</a>
          {#if group.children.length > 0}
            <button
              class="toc-toggle"
              class:toc-toggle-open={isGroupOpen(group.section.id)}
              type="button"
              aria-label={`Toggle ${group.section.title}`}
              aria-expanded={isGroupOpen(group.section.id)}
              on:click={() => toggleGroup(group.section.id)}
            ></button>
          {/if}
        </div>
        {#if group.children.length > 0 && isGroupOpen(group.section.id)}
          <div transition:slide={{ duration: 200 }}>
            {#each group.children as child}
              <a class="toc-nested" class:toc-active={activeSectionId === child.id} href={`#${child.id}`}>{child.title}</a>
            {/each}
          </div>
        {/if}
      </div>
    {/each}
  </aside>

  <div class="doc-content">
    {#if shouldShowPlayground('hello-lk')}
      <Playground embedded />
    {/if}
    <div id="learn-start" class="doc-start-anchor" aria-hidden="true"></div>
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
                        <td>{@html ci === 0 ? highlightInlineCode(cell) : highlightInlineCode(cell)}</td>
                      {/each}
                    </tr>
                  {/each}
                </tbody>
              </table>
            </div>
          {/if}
        {/each}
      </article>

      {#if section.level === 2 && shouldShowPlayground(section.id)}
        <div class="doc-playground-separator"></div>
        <Playground embedded />
      {/if}
    {/each}
  </div>
</section>

{#if showBackToTop}
  <button
    class="floating-back-to-top"
    on:click={() => window.scrollTo({ top: 0, behavior: 'smooth' })}
    aria-label="Back to top"
    transition:fade={{ duration: 150 }}
  >
    <ArrowUp size={18} />
  </button>
{/if}

<style>
  .doc-layout {
    display: grid;
    grid-template-columns: 260px minmax(0, 1fr);
    gap: clamp(1.5rem, 4vw, 3rem);
    width: min(1180px, calc(100vw - 32px));
    margin: 0 auto;
    padding: clamp(3rem, 7vw, 5rem) 0 clamp(5rem, 10vw, 8rem);
    scroll-margin-top: 6.5rem;
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

  .doc-playground-separator {
    height: 1rem;
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
    }

  }

  @media (max-width: 560px) {
    .doc-layout {
      width: min(100% - 24px, 1180px);
    }
  }
</style>
