<script lang="ts">
  import { onMount } from 'svelte'
  import { fade, fly } from 'svelte/transition'
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
  import LearnPage from './components/LearnPage.svelte'
  import StdlibPage from './components/StdlibPage.svelte'
  import Playground from './components/Playground.svelte'
  import { playgroundExamples } from './lib/playgroundExamples'
  import type { PlaygroundSelectionId } from './lib/playgroundExamples'
  import {
    ArrowRight,
    Braces,
    Brackets,
    Check,
    ChevronRight,
    Code2,
    Copy,
    Cpu,
    FileCode2,
    GitBranch,
    Menu,
    Moon,
    Package,
    Play,
    Puzzle,
    Route,
    Shuffle,
    Sun,
    Terminal,
    X,
    Zap,
  } from '@lucide/svelte'

  type NavItem = {
    href: string
    label: 'learn' | 'stdlib' | 'github'
    external?: boolean
  }

  type FeatureItem = {
    icon: Component
    key: 'destructuring' | 'match' | 'optionalChaining' | 'templateStrings' | 'ranges' | 'closures' | 'namedParams' | 'traits' | 'derive' | 'concurrency'
    code: string
  }

  type RuntimeRow = {
    key: 'valueModel' | 'execution' | 'imports' | 'concurrency'
    text: string
  }

  type StdlibGroup = {
    label: string
    modules: string[]
  }

  const navItems: NavItem[] = [
    { href: '/learn', label: 'learn' },
    { href: '/stdlib', label: 'stdlib' },
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

  let locale = $state<Locales | undefined>(initialLocale)
  let currentPath = $state(normalizePath(typeof window === 'undefined' ? '/' : window.location.pathname))
  let currentHash = $state(typeof window === 'undefined' ? '' : window.location.hash)
  let copiedInstall = $state(false)
  let theme = $state<'light' | 'dark'>('dark')
  let compileStep = $state(0)
  let mobileMenuOpen = $state(false)
  let playgroundSource = $state('')
  let playgroundActiveExample = $state<PlaygroundSelectionId>(playgroundExamples[0].id)

  function applyTheme(nextTheme: 'light' | 'dark'): void {
    theme = nextTheme
    localStorage.setItem('theme', nextTheme)
    if (nextTheme === 'dark') {
      document.documentElement.classList.add('dark')
      document.documentElement.classList.remove('light')
    } else {
      document.documentElement.classList.add('light')
      document.documentElement.classList.remove('dark')
    }
  }

  function toggleTheme(): void {
    applyTheme(theme === 'dark' ? 'light' : 'dark')
  }

  const heroCode = `use math;

macro_rules! salute {
  ($name:expr) => { "Hello, " + $name };
}

#[derive(Show)]
struct Point { x: Float, y: Float }

trait Metric { fn mag(self) -> Float; }

impl Metric for Point {
  fn mag(self) -> Float {
    return math.sqrt(self.x * self.x + self.y * self.y);
  }
}

let p = Point { x: 3.0, y: 4.0 };
println("{}, mag = {}", salute!("LK"), p.mag());`

  const installCommand = 'curl -fsSL https://raw.githubusercontent.com/lollipopkit/lk/main/scripts/install.sh | sh'

  const showcaseCode = `struct Rect { w: Int, h: Int }

trait Area {
  fn area(self) -> Int;
}

impl Area for Rect {
  fn area(self) -> Int { return self.w * self.h; }
}

#[derive(Show)]
struct Point { x: Int, y: Int }

let shapes = [
  Rect { w: 3, h: 4 },
  Rect { w: 8, h: 5 },
];

for shape in shapes {
  println("area = {}", shape.area());
}

let p = Point { x: 1, y: 2 };
println("{}", p);  // Point { x: 1, y: 2 }`

  const showcaseOutput = 'area = 12\narea = 40\nPoint { x: 1, y: 2 }'

  const featureItems: FeatureItem[] = [
    { icon: Brackets, key: 'destructuring', code: 'let { "name": name, "roles": [primary, ..rest] } = user;' },
    { icon: Route, key: 'match', code: 'match user {\n  { "role": role } if role in ["editor"] => "staff",\n  _ => "guest"\n}' },
    { icon: Zap, key: 'optionalChaining', code: 'let zip = user?.profile?.address?.zip ?? "000000";' },
    { icon: Braces, key: 'templateStrings', code: '"Ordered ${qty}x ${item}s, total: $${price * qty}"' },
    { icon: Shuffle, key: 'ranges', code: 'for i in 1..=5 { println("val: {}", i); }' },
    { icon: Code2, key: 'closures', code: 'let scale = |x| x * factor;\nlet result = [1, 2, 3].map(scale);' },
    { icon: FileCode2, key: 'namedParams', code: 'fn request(url, { method: String = "GET", timeout: Int })' },
    { icon: Puzzle, key: 'traits', code: 'impl Area for Rect {\n  fn area(self) -> Int { return self.w * self.h; }\n}' },
    { icon: Braces, key: 'derive', code: '#[derive(Show, Debug)]\nstruct User { id: Int, name: String }' },
    { icon: Cpu, key: 'concurrency', code: 'let ch = chan(5, "String");\nspawn(|| send(ch, "pong"));\nlet [ok, msg] = recv(ch);' },
  ]

  const featureCodes: Record<string, string> = {
    destructuring: `let user = { "name": "Mira", "roles": ["admin", "editor"], "active": true };
let { "name": name, "roles": [primary, ..rest] } = user;

println("User: {}, primary role: {}, other roles: {}", name, primary, rest);
return name;`,

    match: `let user = { "role": "editor", "active": true };
let access = match user {
  { "role": "admin", "active": true } => "superuser",
  { "role": role } if role in ["editor", "writer"] => "staff",
  _ => "guest"
};

println("User access level: {}", access);
return access;`,

    optionalChaining: `let user = { "profile": { "name": "Alice" } };
let name = user?.profile?.name ?? "guest";
let age = user?.profile?.age ?? 18;

println("Name: {}, Age: {}", name, age);
return name;`,

    templateStrings: `let item = "widget";
let qty = 3;
let price = 12.5;

let receipt = "Ordered \${qty}x \${item}s, total: \$\${price * qty}";
println("Receipt: {}", receipt);
return receipt;`,

    ranges: `let total = 0;
for i in 1..=5 {
  total += i;
}

let step_list = 0..10..2; // [0, 2, 4, 6, 8]
println("Sum 1..=5: {}, Stepped list: {}", total, step_list);
return total;`,

    closures: `let factor = 3;
let scale = |x| x * factor;
let result = [1, 2, 3].map(scale);

println("Scaled list: {}", result);
return result;`,

    namedParams: `fn request(url, { method: String = "GET", timeout_ms: Int = 5000 }) {
  println("Requesting {} via {} (timeout: {}ms)", url, method, timeout_ms);
  return true;
}

return request("https://lk-lang.org", timeout_ms: 1000);`,

    traits: `struct Rect { w: Int, h: Int }

trait Area {
  fn area(self) -> Int;
}

impl Area for Rect {
  fn area(self) -> Int {
    return self.w * self.h;
  }
}

let r = Rect { w: 5, h: 4 };
println("Rect area: {}", r.area());
return r.area();`,

    derive: `#[derive(Show, Debug)]
struct User { id: Int, name: String }

let u = User { id: 42, name: "Bob" };
println("User object: {}", u);
return u.name;`,

    concurrency: `let ch = chan(5, "String");
spawn(|| {
  send(ch, "pong");
});

println("Waiting for message...");
let [ok, msg] = recv(ch);
println("Received: {}, ok={}", msg, ok);
return msg;`
  }

  function tryCodeSnippet(key: string): void {
    const code = featureCodes[key]
    if (code) {
      playgroundSource = code
      playgroundActiveExample = 'custom'
      scrollToSection('showcase')
    }
  }

  const stdlibGroups: StdlibGroup[] = [
    { label: 'Core', modules: ['math', 'string', 'iter', 'stream'] },
    { label: 'Data', modules: ['list', 'map', 'set', 'bytes', 'slice'] },
    { label: 'IO & FS', modules: ['io', 'fs', 'path', 'env', 'process', 'os'] },
    { label: 'Encoding', modules: ['encoding', 'hash', 'regex', 'base64'] },
    { label: 'Network', modules: ['http', 'net', 'datetime', 'time'] },
    { label: 'Concurrency', modules: ['task', 'chan', 'random', 'uuid'] },
  ]

  const runtimeRows = $derived(getRuntimeRows(locale))

  function getRuntimeRows(activeLocale: Locales | undefined): RuntimeRow[] {
    if (activeLocale === 'zh-CN') {
      return [
        { key: 'valueModel', text: 'String、Int、Float、Bool、Nil、List、Map、Function、Object、Iterator、Stream、Task、Channel' },
        { key: 'execution', text: 'REPL、源文件执行、native 可执行文件构建、bytecode 产物、类型检查诊断' },
        { key: 'imports', text: '标准库模块、选择性导入、别名、命名空间导入、安全文件模块和 package workspace' },
        { key: 'concurrency', text: 'Feature-gated spawn、channel、send、recv、select、task、stream' },
      ]
    }
    return [
      { key: 'valueModel', text: 'String, Int, Float, Bool, Nil, List, Map, Function, Object, Iterator, Stream, Task, Channel' },
      { key: 'execution', text: 'REPL, source-file execution, native executables, bytecode artifacts, type-check diagnostics' },
      { key: 'imports', text: 'Stdlib modules, selected imports, aliases, namespace imports, file modules, package workspaces' },
      { key: 'concurrency', text: 'Feature-gated spawn, channels, send, recv, select, task, stream' },
    ]
  }

  function getNavLabel(item: NavItem): string {
    return $LL.nav[item.label]()
  }

  function normalizePath(path: string): string {
    if (path === '/LANG.md' || path === '/try' || path === '/spec') return '/'
    return path
  }

  function isCurrentNavItem(item: NavItem): boolean {
    if (item.external) return false
    const url = new URL(item.href, typeof window === 'undefined' ? 'http://localhost' : window.location.origin)
    const path = normalizePath(url.pathname)
    return path === currentPath
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

  function navigate(path: string): void {
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

  function scrollToSection(id: string): void {
    document.getElementById(id)?.scrollIntoView({ behavior: 'smooth', block: 'start' })
  }

  async function copyInstallCommand(): Promise<void> {
    try {
      await navigator.clipboard.writeText(installCommand)
      copiedInstall = true
      window.setTimeout(() => { copiedInstall = false }, 2000)
    } catch { /* ignore */ }
  }

  onMount(() => {
    const nextLocale = locale || getInitialLocale()
    applyLocale(nextLocale)

    playgroundSource = playgroundExamples[0].code
    playgroundActiveExample = playgroundExamples[0].id

    const savedTheme = localStorage.getItem('theme') as 'light' | 'dark' | null
    if (savedTheme) {
      applyTheme(savedTheme)
    } else {
      const preferred = window.matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light'
      applyTheme(preferred)
    }

    const compileInterval = window.setInterval(() => {
      // Loop compileStep from 1 -> 2 -> 3 -> 0 (finished/all completed) -> 1
      compileStep = (compileStep + 1) % 4
    }, 1500)

    if (window.location.pathname === '/try' || window.location.pathname === '/LANG.md' || window.location.pathname === '/spec') {
      window.history.replaceState({}, '', pathWithLocale('/'))
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
    return () => {
      window.removeEventListener('popstate', handlePopstate)
      window.clearInterval(compileInterval)
    }
  })

  $effect(() => {
    if (!locale) return
    document.documentElement.lang = $LL.meta.lang()
    document.title = $LL.meta.title()
    document.querySelector('meta[name="description"]')?.setAttribute('content', $LL.meta.description())
  })
</script>

{#if locale}
<main class="site">
  <header class="site-nav" id="top">
    <a class="brand" href="/" onclick={() => navigate('/')}>
      <span class="brand-mark">LK</span>
    </a>
    <nav aria-label="Primary navigation">
      {#each navItems as item}
        {#if item.external}
          <a href={item.href} target="_blank" rel="noreferrer">{getNavLabel(item)}</a>
        {:else}
          <a href={item.href} aria-current={isCurrentNavItem(item) ? 'page' : undefined} onclick={() => navigate(item.href)}>{getNavLabel(item)}</a>
        {/if}
      {/each}
    </nav>
    <div class="nav-actions">
      <button
        class="icon-btn theme-toggle-btn"
        type="button"
        onclick={toggleTheme}
        aria-label="Toggle dark mode"
      >
        {#if theme === 'dark'}
          <Sun size={17} />
        {:else}
          <Moon size={17} />
        {/if}
      </button>
      <label class="language-switcher">
        <span class="sr-only">{$LL.nav.languageLabel()}</span>
        <select
          aria-label={$LL.nav.languageLabel()}
          value={locale}
          onchange={handleLocaleChange}
        >
          {#each locales as item}
            <option value={item.code}>{item.label}</option>
          {/each}
        </select>
      </label>
      <button
        class="icon-btn mobile-menu-btn"
        type="button"
        onclick={() => mobileMenuOpen = true}
        aria-label="Open menu"
      >
        <Menu size={18} />
      </button>
    </div>
  </header>

  {#if mobileMenuOpen}
    <div
      class="mobile-menu-overlay"
      transition:fade={{ duration: 150 }}
      onclick={(e) => { if (e.target === e.currentTarget) mobileMenuOpen = false; }}
      onkeydown={(e) => { if (e.key === 'Escape') mobileMenuOpen = false; }}
      role="button"
      tabindex="-1"
      aria-label="Close menu"
    >
      <nav class="mobile-nav">
        <div class="mobile-nav-header">
          <span class="brand-mark">LK</span>
          <button class="icon-btn close-btn" type="button" onclick={() => mobileMenuOpen = false} aria-label="Close menu">
            <X size={18} />
          </button>
        </div>
        <div class="mobile-nav-links">
          {#each navItems as item}
            {#if item.external}
              <a href={item.href} target="_blank" rel="noreferrer" onclick={() => mobileMenuOpen = false}>{getNavLabel(item)}</a>
            {:else}
              <a href={item.href} class:active={isCurrentNavItem(item)} onclick={() => { navigate(item.href); mobileMenuOpen = false; }}>{getNavLabel(item)}</a>
            {/if}
          {/each}
        </div>
      </nav>
    </div>
  {/if}

  {#key currentPath}
    <div class="route-shell" in:fly={{ y: 14, duration: 260 }} out:fade={{ duration: 120 }}>
    {#if currentPath === '/learn'}
      <LearnPage {locale} />
    {:else if currentPath === '/stdlib'}
      <StdlibPage {locale} />
    {:else}
      <section class="hero" aria-labelledby="hero-title">
      <div class="hero-copy">
        <p class="eyebrow">{$LL.hero.eyebrow()}</p>
        <h1 id="hero-title">{@html $LL.hero.title().replace(/\n/g, '<br>')}</h1>
        <p class="hero-subtitle">{$LL.hero.subtitle()}</p>
        <div class="hero-actions">
          <a class="btn btn-primary" href="/learn" onclick={() => navigate('/learn')}>
            <Play size={18} />
            {$LL.hero.primaryAction()}
          </a>
          <a class="btn btn-secondary" href="#features" onclick={() => scrollToSection('features')}>
            {$LL.hero.secondaryAction()}
            <ArrowRight size={18} />
          </a>
        </div>
      </div>
      <div class="hero-visual" aria-label={$LL.hero.previewLabel()}>
        <div class="terminal-window">
          <div class="terminal-topbar">
            <span></span><span></span><span></span>
            <strong>request.lk</strong>
          </div>
          <pre><code>{@html highlightLkCode(heroCode)}</code></pre>
        </div>
        <div class="compile-strip">
          <!-- Path 1: Direct VM Run -->
          <div class="compile-path">
            <div class="path-label">Run</div>
            <div class="path-steps">
              <span class:active-step={compileStep === 1} class:done-step={compileStep > 1 || compileStep === 0}>
                <Terminal size={14} /> lk request.lk
              </span>
              <ChevronRight size={14} class={`strip-chevron ${compileStep === 1 || compileStep === 2 ? 'active-chevron' : ''}`} />
              <span class:active-step={compileStep === 2} class:done-step={compileStep > 2 || compileStep === 0}>
                <Cpu size={14} class={compileStep === 2 ? 'spinning' : ''} /> bytecode VM
              </span>
            </div>
          </div>

          <!-- Path 2: LLVM Compile -->
          <div class="compile-path">
            <div class="path-label">Compile</div>
            <div class="path-steps">
              <span class:active-step={compileStep === 1} class:done-step={compileStep > 1 || compileStep === 0}>
                <Terminal size={14} /> lk compile request.lk
              </span>
              <ChevronRight size={14} class={`strip-chevron ${compileStep === 1 || compileStep === 2 ? 'active-chevron' : ''}`} />
              <span class:active-step={compileStep === 2} class:done-step={compileStep > 2 || compileStep === 0}>
                <Cpu size={14} class={compileStep === 2 ? 'spinning' : ''} /> LLVM compiler
              </span>
            </div>
          </div>
        </div>
      </div>
    </section>

    <section class="feature-section" id="features" aria-labelledby="features-title">
      <div class="section-kicker">
        <span>01</span>
        <p>{$LL.feature.kicker()}</p>
      </div>
      <h2 id="features-title">{$LL.feature.title()}</h2>
      <div class="feature-grid">
        {#each featureItems as item, index}
          {@const Icon = item.icon}
          <article style={`--i:${index}`} class="feature-card">
            <div class="feature-card-header">
              <Icon size={20} />
              <h3>{$LL.feature.groups[item.key].title()}</h3>
            </div>
            <code>{@html highlightLkCode(item.code)}</code>
            <button
              class="btn btn-secondary try-btn"
              onclick={() => tryCodeSnippet(item.key)}
              aria-label={`Try ${$LL.feature.groups[item.key].title()} in playground`}
            >
              <span>Try</span>
              <ArrowRight size={14} />
            </button>
          </article>
        {/each}
      </div>
    </section>

    <section class="showcase-section" id="showcase" aria-labelledby="showcase-title">
      <div class="section-kicker">
        <span>02</span>
        <p>{$LL.showcase.kicker()}</p>
      </div>
      <h2 id="showcase-title">{$LL.showcase.title()}</h2>
      <div class="showcase-playground">
        <Playground embedded bind:source={playgroundSource} bind:activeExample={playgroundActiveExample} />
      </div>
    </section>

    <section class="runtime-section" id="runtime" aria-labelledby="runtime-title">
      <div class="section-kicker">
        <span>03</span>
        <p>{$LL.runtime.kicker()}</p>
      </div>
      <h2 id="runtime-title">{$LL.runtime.title()}</h2>
      <div class="runtime-cards">
        {#each runtimeRows as row}
          <article class="runtime-card">
            <strong>{$LL.runtime.rows[row.key]()}</strong>
            <span>{row.text}</span>
          </article>
        {/each}
      </div>
    </section>

    <section class="stdlib-section" id="stdlib" aria-labelledby="stdlib-title">
      <div class="section-kicker">
        <span>04</span>
        <p>{$LL.stdlib.kicker()}</p>
      </div>
      <h2 id="stdlib-title">{$LL.stdlib.title()}</h2>
      <div class="stdlib-groups">
        {#each stdlibGroups as group}
          <div class="stdlib-group">
            <h3>{group.label}</h3>
            <div class="stdlib-tags">
              {#each group.modules as module}
                <span>{module}</span>
              {/each}
            </div>
          </div>
        {/each}
      </div>
    </section>

    <section class="start-section" id="start" aria-labelledby="start-title">
      <div class="start-copy">
        <div class="section-kicker">
          <span>05</span>
          <p>{$LL.start.kicker()}</p>
        </div>
        <h2 id="start-title">{$LL.start.title()}</h2>
      </div>
      <div class="start-actions">
        <div class="install-row">
          <code class="install-cmd" aria-label={installCommand}>
            <span class="shell-command">curl</span>
            <span class="shell-flag">-fsSL</span>
            <span class="shell-url">https://raw.githubusercontent.com/lollipopkit/lk/main/scripts/install.sh</span>
            <span class="shell-pipe">|</span>
            <span class="shell-command">sh</span>
          </code>
          <button class="icon-btn copy-btn" onclick={copyInstallCommand} aria-label="Copy">
            {#if copiedInstall}
              <Check size={16} />
            {:else}
              <Copy size={16} />
            {/if}
          </button>
        </div>
        <div class="command-grid">
          <a class="cmd-link" href="/learn" onclick={() => navigate('/learn')}>
            <Code2 size={16} />
            <span>lk FILE</span>
          </a>
          <a class="cmd-link" href="/learn" onclick={() => navigate('/learn')}>
            <Package size={16} />
            <span>lk compile FILE</span>
          </a>
          <a class="cmd-link" href="/learn" onclick={() => navigate('/learn')}>
            <GitBranch size={16} />
            <span>lk pkg tree</span>
          </a>
        </div>
      </div>
      </section>
    {/if}
    </div>
  {/key}

  <footer class="site-footer">
    <span>{$LL.footer.brand()}</span>
    <a href="/" onclick={() => navigate('/')}>{$LL.footer.home()}</a>
    <a href="/learn" onclick={() => navigate('/learn')}>{$LL.footer.learn()}</a>
    <a href="/stdlib" onclick={() => navigate('/stdlib')}>{$LL.footer.stdlib()}</a>
  </footer>
</main>
{/if}
