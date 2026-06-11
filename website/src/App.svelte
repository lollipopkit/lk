<script lang="ts">
  import { onMount } from 'svelte'
  import type { Component } from 'svelte'
  import LL, { setLocale } from './i18n/i18n-svelte'
  import { loadLocale } from './i18n/i18n-util.sync'
  import type { Locales } from './i18n/i18n-types'
  import {
    getInitialLocale,
    locales,
    localeStorageKey,
    syncLocaleToUrl,
  } from './lib/i18n'
  import { highlightLkCode } from './lib/highlight'
  import { parseDocMarkdown, buildDocNav, type DocSection, type DocNavGroup, type DocBlock } from './lib/docPage'
  import Playground from './components/Playground.svelte'
  import LearnPage from './components/LearnPage.svelte'
  import StdlibPage from './components/StdlibPage.svelte'
  import {
    ArrowRight,
    Boxes,
    Braces,
    Check,
    ChevronRight,
    Code2,
    Cpu,
    FileCode2,
    GitBranch,
    Package,
    Play,
    Puzzle,
    Route,
    Terminal,
  } from '@lucide/svelte'
  import langDocument from './spec/LANG.md?raw'
  import langZhDocument from './spec/LANG_zh.md?raw'

  type NavItem = {
    href: string
    label: 'try' | 'learn' | 'stdlib' | 'spec' | 'github'
    external?: boolean
  }

  type FeatureGroup = {
    icon: Component
    key: 'expression' | 'collections' | 'patterns' | 'traits'
    title: string
    body: string
    sample: string
  }

  type Example = {
    label: string
    code: string
  }

  type RuntimeRow = {
    key: 'valueModel' | 'execution' | 'imports' | 'concurrency'
    text: string
  }

  const navItems: NavItem[] = [
    { href: '/learn', label: 'learn' },
    { href: '/stdlib', label: 'stdlib' },
    { href: '/spec', label: 'spec' },
    { href: 'https://github.com/lollipopkit/lk', label: 'github', external: true },
  ]

  function getLocaleBeforeRender(): Locales | undefined {
    if (typeof window === 'undefined') return undefined
    return getInitialLocale()
  }

  const initialLocale = getLocaleBeforeRender()

  if (initialLocale) {
    loadLocale(initialLocale)
    setLocale(initialLocale)
  }

  let locale: Locales | undefined = initialLocale
  let currentPath = normalizePath(typeof window === 'undefined' ? '/' : window.location.pathname)
  let currentHash = typeof window === 'undefined' ? '' : window.location.hash

  const heroCode = `use { std } from io;
use { json } from encoding;

let data = json.parse(std.read_to_string(std.stdin()));

match data.req {
  { "user": { "id": id }, ..rest } if id in [1, 2, 3] => {
    println("accepted {}", id);
  },
  _ => panic("unknown request"),
};`

  const featureGroups: FeatureGroup[] = [
    {
      icon: Braces,
      key: 'expression',
      title: 'Modern expression core',
      body: 'Template strings, nullish coalescing, right-associative ternaries, optional chaining, range literals, bitwise operators, and first-class closures live in the same compact expression grammar.',
      sample: '"Hello, ${user.name}" ?? "guest"',
    },
    {
      icon: Boxes,
      key: 'collections',
      title: 'Lists and maps built in',
      body: 'Heterogeneous collections support negative indexing, slicing, spread, dot access, compound assignment, list subtraction, map merges, and meta-method dispatch.',
      sample: 'scores[-1] + { winner: user.name }.winner',
    },
    {
      icon: Route,
      key: 'patterns',
      title: 'Pattern matching everywhere',
      body: 'Match arms, if let, while let, let destructuring, and for-loop destructuring cover literals, ranges, maps, lists, rest bindings, or-patterns, and guards.',
      sample: 'if let { "id": id, ..rest } = payload { ... }',
    },
    {
      icon: Puzzle,
      key: 'traits',
      title: 'Structs, traits, and methods',
      body: 'Define records, implement traits, call methods through direct properties or runtime meta-method dispatch, and let display methods format values automatically.',
      sample: 'impl Area for Rect { fn area(self) -> Int { ... } }',
    },
  ]

  const stdlib: string[] = [
    'math',
    'string',
    'list',
    'map',
    'iter',
    'stream',
    'datetime',
    'os',
    'fs',
    'path',
    'env',
    'process',
    'io',
    'encoding',
    'hash',
    'regex',
    'random',
    'uuid',
    'http',
    'net',
    'task',
    'chan',
    'time',
  ]

  const examples: Example[] = [
    {
      label: 'Named parameters',
      code: `fn draw_rect(x: Int, y: Int, { width: Int, height: Int? = 100 }) -> Int {
  return width * (height ?? 0);
}`,
    },
      {
        label: 'Use forms',
        code: `use math as m;
use { file, std } from io;
use { abs, sqrt } from math;
use * as config from "config/app";`,
      },
    {
      label: 'Collection pipelines',
      code: `use iter;

let total = iter.reduce(iter.range(0, 10, 2), 0, |acc, n| acc + n);`,
    },
  ]

  $: runtimeRows = getRuntimeRows(locale)
  $: compileStripRuntime = locale === 'zh-CN' ? 'bytecode VM' : 'bytecode VM'

  function getRuntimeRows(activeLocale: Locales | undefined): RuntimeRow[] {
    if (activeLocale === 'zh-CN') {
      return [
        { key: 'valueModel', text: 'String、Int、Float、Bool、Nil、List、Map、Function、Object、Iterator、Stream、Task、Channel' },
        {
          key: 'execution',
          text: 'REPL、源文件执行、native 可执行文件构建、bytecode 产物、类型检查诊断，以及面向脚本和嵌入场景的 CLI 工具',
        },
        { key: 'imports', text: '标准库模块、选择性导入、别名、命名空间导入、安全相对文件模块和 package workspace' },
        { key: 'concurrency', text: 'Feature-gated spawn、channel、send、recv、select、task、stream 和阻塞收集 helper' },
      ]
    }

    return [
      { key: 'valueModel', text: 'String, Int, Float, Bool, Nil, List, Map, Function, Object, Iterator, Stream, Task, Channel' },
      {
        key: 'execution',
        text: 'REPL, source-file execution, native executable builds, bytecode artifacts, type-check diagnostics, and CLI tooling for scripting and embedding',
      },
      { key: 'imports', text: 'Stdlib modules, selected imports, aliases, namespace imports, sanitized relative file modules, and package workspaces' },
      { key: 'concurrency', text: 'Feature-gated spawn, channels, send, recv, select, task, stream, and blocking collection helpers' },
    ]
  }

  function getNavLabel(item: NavItem): string {
    return $LL.nav[item.label]()
  }

  $: activeLangDocument = locale === 'zh-CN' ? langZhDocument : langDocument
  $: specSections = parseDocMarkdown(activeLangDocument)
  $: specNav = buildDocNav(specSections)

  function normalizePath(path: string): string {
    if (path === '/LANG.md' || path === '/try') return '/spec'
    return path
  }

  function isCurrentNavItem(item: NavItem): boolean {
    if (item.external) return false
    const url = new URL(item.href, typeof window === 'undefined' ? 'http://localhost' : window.location.origin)
    const path = normalizePath(url.pathname)
    if (path !== currentPath) return false
    if (item.label === 'spec' && currentHash === '#examples') return false
    if (item.label === 'try') return currentPath === '/spec' && currentHash === '#examples'
    return true
  }

  function pathWithLocale(path: string): string {
    if (!locale) return path
    const url = new URL(path, window.location.origin)
    url.searchParams.set('lang', locale)
    return `${url.pathname}${url.search}${url.hash}`
  }

  function applyLocale(nextLocale: Locales): void {
    locale = nextLocale
    loadLocale(nextLocale)
    setLocale(nextLocale)
    localStorage.setItem(localeStorageKey, nextLocale)
  }

  function handleLocaleChange(event: Event): void {
    const nextLocale = (event.currentTarget as HTMLSelectElement).value as Locales
    applyLocale(nextLocale)
    syncLocaleToUrl(nextLocale)
  }

  function navigate(event: MouseEvent, path: string): void {
    event.preventDefault()
    if (typeof window === 'undefined') return
    window.history.pushState({}, '', pathWithLocale(path))
    currentPath = normalizePath(window.location.pathname)
    currentHash = window.location.hash
    if (currentHash) {
      window.setTimeout(() => {
        document.getElementById(currentHash.slice(1))?.scrollIntoView({ behavior: 'smooth', block: 'start' })
      }, 0)
      return
    }
    window.scrollTo({ top: 0, behavior: 'smooth' })
  }

  function navigateHomeSection(event: MouseEvent, id: string): void {
    event.preventDefault()
    if (typeof window === 'undefined') return
    if (currentPath !== '/') {
      window.history.pushState({}, '', pathWithLocale('/'))
      currentPath = '/'
      window.setTimeout(() => {
        document.getElementById(id)?.scrollIntoView({ behavior: 'smooth', block: 'start' })
      }, 0)
      return
    }
    document.getElementById(id)?.scrollIntoView({ behavior: 'smooth', block: 'start' })
  }

  function highlightInlineCode(text: string): string {
    return text
      .replace(/&/g, '&amp;')
      .replace(/</g, '&lt;')
      .replace(/>/g, '&gt;')
      .replace(/`([^`]+)`/g, '<code class="inline-code">$1</code>')
  }

  onMount(() => {
    const nextLocale = locale || getInitialLocale()
    applyLocale(nextLocale)
    if (window.location.pathname === '/try') {
      window.history.replaceState({}, '', pathWithLocale('/spec#examples'))
    } else {
      syncLocaleToUrl(nextLocale)
    }
    currentPath = normalizePath(window.location.pathname)
    currentHash = window.location.hash
    if (currentHash) {
      window.setTimeout(() => {
        document.getElementById(currentHash.slice(1))?.scrollIntoView({ block: 'start' })
      }, 0)
    }

    const handlePopstate = () => {
      currentPath = normalizePath(window.location.pathname)
      currentHash = window.location.hash
    }

    window.addEventListener('popstate', handlePopstate)
    return () => window.removeEventListener('popstate', handlePopstate)
  })

  $: if (locale) {
    document.documentElement.lang = $LL.meta.lang()
    document.title = $LL.meta.title()
    document
      .querySelector('meta[name="description"]')
      ?.setAttribute('content', $LL.meta.description())
  }

  function scrollToSection(event: MouseEvent, id: string): void {
    event.preventDefault()
    document.getElementById(id)?.scrollIntoView({ behavior: 'smooth', block: 'start' })
  }
</script>

{#if locale}
<main class="site">
  <header class="site-nav" id="top">
    <a class="brand" href="/" on:click={(event) => navigate(event, '/')}>
      <span class="brand-mark">LK</span>
    </a>
    <nav aria-label="Primary navigation">
      {#each navItems as item}
        {#if item.external}
          <a href={item.href} target="_blank" rel="noreferrer">{getNavLabel(item)}</a>
        {:else}
          <a href={item.href} aria-current={isCurrentNavItem(item) ? 'page' : undefined} on:click={(event) => navigate(event, item.href)}>{getNavLabel(item)}</a>
        {/if}
      {/each}
    </nav>
    <div class="nav-actions">
      <label class="language-switcher">
        <span class="sr-only">{$LL.nav.languageLabel()}</span>
        <select
          aria-label={$LL.nav.languageLabel()}
          value={locale}
          on:change={handleLocaleChange}
        >
          {#each locales as item}
            <option value={item.code}>{item.label}</option>
          {/each}
        </select>
      </label>
    </div>
  </header>

  {#if currentPath === '/learn'}
    <LearnPage {locale} />
  {:else if currentPath === '/stdlib'}
    <StdlibPage {locale} />
  {:else if currentPath === '/spec'}
    <section class="spec-layout" aria-label="LK language specification">
      <aside class="spec-toc" aria-label="Specification table of contents">
        <strong>{$LL.spec.toc()}</strong>
        {#each specNav as group}
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

      <div class="spec-content">
        <Playground embedded />
        <div id="spec-start" class="spec-start-anchor" aria-hidden="true"></div>
        {#each specSections as section}
          <article class={`spec-card spec-level-${section.level}`} id={section.id}>
            {#if section.level <= 2}
              <p class="spec-index">{String(specSections.indexOf(section) + 1).padStart(2, '0')}</p>
            {/if}
            {#if section.level === 2}
              <h2>{section.title}</h2>
            {:else}
              <h3>{section.title}</h3>
            {/if}

            {#each section.blocks as block}
              {#if block.type === 'paragraph'}
                <p>{block.text}</p>
              {:else if block.type === 'list'}
                <ul>
                  {#each block.items as item}
                    <li>{@html highlightInlineCode(item)}</li>
                  {/each}
                </ul>
              {:else if block.type === 'code'}
                <pre><code>{@html highlightLkCode(block.text)}</code></pre>
              {:else if block.type === 'table'}
                <div class="spec-table-wrapper">
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
                            <td>{@html ci === 0 ? highlightInlineCode(cell) : cell}</td>
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
  {:else}
    <section class="hero" aria-labelledby="hero-title">
      <div class="hero-copy">
        <p class="eyebrow">{$LL.hero.eyebrow()}</p>
        <h1 id="hero-title">{$LL.hero.title()}</h1>
        <p class="hero-subtitle">
          {$LL.hero.subtitle()}
        </p>
        <div class="hero-actions" aria-label="Hero actions">
          <a class="btn btn-primary" href="/learn" on:click={(event) => navigate(event, '/learn')}>
            <Play size={18} />
            {$LL.nav.learn()}
          </a>
          <a class="btn btn-secondary" href="#language" on:click={(event) => scrollToSection(event, 'language')}>
            {$LL.hero.secondaryAction()}
            <ArrowRight size={18} />
          </a>
        </div>
      </div>

      <div class="hero-visual" aria-label={$LL.hero.previewLabel()}>
        <div class="terminal-window">
          <div class="terminal-topbar">
            <span></span>
            <span></span>
            <span></span>
            <strong>request.lk</strong>
          </div>
          <pre><code>{@html highlightLkCode(heroCode)}</code></pre>
        </div>
        <div class="compile-strip">
          <span><Terminal size={16} /> lk request.lk</span>
          <ChevronRight size={16} />
          <span><Cpu size={16} /> {compileStripRuntime}</span>
          <ChevronRight size={16} />
          <span><Check size={16} /> diagnostics</span>
        </div>
      </div>
    </section>

    <section class="feature-band" id="language" aria-labelledby="language-title">
      <div class="section-kicker">
        <span>01</span>
        <p>{$LL.feature.kicker()}</p>
      </div>
      <div class="section-heading">
        <h2 id="language-title">{$LL.feature.title()}</h2>
        <p>
          {$LL.feature.subtitle()}
        </p>
      </div>

      <div class="feature-grid">
        {#each featureGroups as item, index}
          <article class:feature-wide={index === 0} style={`--i:${index}`}>
            <svelte:component this={item.icon} size={22} />
            <h3>{$LL.feature.groups[item.key].title()}</h3>
            <p>{$LL.feature.groups[item.key].body()}</p>
            <code>{@html highlightLkCode(item.sample)}</code>
          </article>
        {/each}
      </div>
    </section>

    <section class="runtime-section" id="runtime" aria-labelledby="runtime-title">
      <div class="runtime-panel">
        <div class="section-kicker">
          <span>02</span>
          <p>{$LL.runtime.kicker()}</p>
        </div>
        <h2 id="runtime-title">{$LL.runtime.title()}</h2>
        <p>
          {$LL.runtime.subtitle()}
        </p>
      </div>
      <div class="runtime-table" aria-label="Runtime capabilities">
        {#each runtimeRows as row}
          <div class="runtime-row">
            <strong>{$LL.runtime.rows[row.key]()}</strong>
            <span>{row.text}</span>
          </div>
        {/each}
      </div>
    </section>

    <section class="stdlib-section" id="stdlib" aria-labelledby="stdlib-title">
      <div class="section-heading compact">
        <div class="section-kicker">
          <span>03</span>
          <p>{$LL.stdlib.kicker()}</p>
        </div>
        <h2 id="stdlib-title">{$LL.stdlib.title()}</h2>
      </div>
      <div class="module-cloud" aria-label="Standard library modules">
        {#each stdlib as module}
          <span>{module}</span>
        {/each}
      </div>
    </section>

    <section class="examples-section" aria-labelledby="examples-title">
      <div class="section-heading">
        <h2 id="examples-title">{$LL.examples.title()}</h2>
        <p>
          {$LL.examples.subtitle()}
        </p>
      </div>
      <div class="example-stack">
        {#each examples as example}
          <article>
            <header>
              <FileCode2 size={18} />
            <h3>{example.label === 'Named parameters' ? $LL.examples.namedParameters() : example.label === 'Use forms' ? $LL.examples.importForms() : $LL.examples.collectionPipelines()}</h3>
            </header>
            <pre><code>{@html highlightLkCode(example.code)}</code></pre>
          </article>
        {/each}
      </div>
    </section>

    <section class="start-section" id="start" aria-labelledby="start-title">
      <div>
        <div class="section-kicker">
          <span>04</span>
          <p>{$LL.start.kicker()}</p>
        </div>
        <h2 id="start-title">{$LL.start.title()}</h2>
      </div>
      <div class="command-grid">
        <code><Terminal size={16} /> lk</code>
        <code><Code2 size={16} /> lk check FILE</code>
        <code><Package size={16} /> lk compile FILE</code>
        <code><GitBranch size={16} /> lk pkg tree</code>
      </div>
    </section>
  {/if}

  <footer class="site-footer">
    <span>{$LL.footer.brand()}</span>
    <a href="/" on:click={(event) => navigate(event, '/')}>{$LL.footer.home()}</a>
    <a href="/learn" on:click={(event) => navigate(event, '/learn')}>{$LL.footer.learn()}</a>
    <a href="/stdlib" on:click={(event) => navigate(event, '/stdlib')}>{$LL.footer.stdlib()}</a>
    <a href="/spec" on:click={(event) => navigate(event, '/spec')}>{$LL.footer.spec()}</a>
  </footer>
</main>
{/if}
