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
  import {
    ArrowRight,
    Boxes,
    Braces,
    Cable,
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
    label: 'performance' | 'spec' | 'github'
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

  type PerformanceCard = {
    icon: Component
    title: string
    body: string
  }

  type TechnicalSection = {
    kicker: string
    title: string
    body: string
    items: string[]
  }

  type SpecBlock =
    | { type: 'paragraph'; text: string }
    | { type: 'list'; items: string[] }
    | { type: 'code'; text: string }

  type SpecSection = {
    id: string
    level: number
    title: string
    blocks: SpecBlock[]
  }

  type SpecNavGroup = {
    section: SpecSection
    children: SpecSection[]
  }

  const navItems: NavItem[] = [
    { href: '/performance', label: 'performance' },
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

  const heroCode = `import io;
import json;

let data = json.parse(io.read());

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
    'io',
    'json',
    'yaml',
    'toml',
    'tcp',
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
      label: 'Import forms',
      code: `import math as m;
import { abs, sqrt } from math;
import * as config from "config/app";`,
    },
    {
      label: 'Collection pipelines',
      code: `import iter;

let total = iter.reduce(iter.range(0, 10, 2), 0, |acc, n| acc + n);`,
    },
  ]

  $: runtimeRows = getRuntimeRows(locale)
  $: performanceCards = getPerformanceCards(locale)
  $: technicalSections = getTechnicalSections(locale)
  $: technicalCommands = getTechnicalCommands(locale)
  $: performanceMetric =
    locale === 'zh-CN'
      ? '当前 workload suite：默认 bytecode VM 最新复验约 0.79-0.81x vs Lua；supported native/AOT shapes 历史约 0.35x vs Lua。'
      : 'Current workload suite: the latest default bytecode VM validation is about 0.79-0.81x vs Lua; supported native/AOT shapes have historically measured about 0.35x vs Lua.'
  $: compileStripRuntime = locale === 'zh-CN' ? 'bytecode VM' : 'bytecode VM'

  function getRuntimeRows(activeLocale: Locales | undefined): RuntimeRow[] {
    if (activeLocale === 'zh-CN') {
      return [
        { key: 'valueModel', text: 'String、Int、Float、Bool、Nil、List、Map、Function、Object、Iterator、Stream、Task、Channel' },
        {
          key: 'execution',
          text: 'REPL、默认 VM 源文件执行、模块产物、类型检查诊断、可选 cached native executable，以及支持 shape 的 true-native LLVM AOT',
        },
        { key: 'imports', text: '标准库模块、选择性导入、别名、命名空间导入、安全相对文件模块和 package workspace' },
        { key: 'concurrency', text: 'Feature-gated spawn、channel、send、recv、select、task、stream 和阻塞收集 helper' },
      ]
    }

    return [
      { key: 'valueModel', text: 'String, Int, Float, Bool, Nil, List, Map, Function, Object, Iterator, Stream, Task, Channel' },
      {
        key: 'execution',
        text: 'REPL, default VM source execution, module artifacts, type-check diagnostics, optional cached native executables, and true-native LLVM AOT for supported shapes',
      },
      { key: 'imports', text: 'Stdlib modules, selected imports, aliases, namespace imports, sanitized relative file modules, and package workspaces' },
      { key: 'concurrency', text: 'Feature-gated spawn, channels, send, recv, select, task, stream, and blocking collection helpers' },
    ]
  }

  function getPerformanceCards(activeLocale: Locales | undefined): PerformanceCard[] {
    if (activeLocale === 'zh-CN') {
      return [
        {
          icon: Cpu,
          title: '直接执行默认 VM',
          body: '直接运行 `lk FILE` 时，LK 使用 bytecode VM，行为稳定、易诊断，适合日常脚本执行。',
        },
        {
          icon: Terminal,
          title: 'native 是显式选择',
          body: '`LK_NATIVE_RUN=1 lk FILE` 可启用 cached native executable；`lk compile exe FILE` 可显式生成 AOT executable。',
        },
        {
          icon: Braces,
          title: 'Opcode specialization',
          body: '当前 opcode 与 lowering 优化面向通用 operand shape，用于减少解释器里的重复 materialization、zero/small-int branch 和临时寄存器搬运。',
        },
        {
          icon: Code2,
          title: 'AOT 用于稳定交付',
          body: 'VM、cached native 和 AOT 使用同一套 workload checksum 校验语义一致性。',
        },
      ]
    }

    return [
      {
        icon: Cpu,
        title: 'Direct execution defaults to the VM',
        body: 'Running `lk FILE` uses the bytecode VM, keeping everyday script execution stable, diagnosable, and easy to reproduce.',
      },
      {
        icon: Terminal,
        title: 'The VM remains deterministic',
        body: 'The bytecode VM is the default correctness oracle and diagnostic execution path. Set `LK_NATIVE_RUN=1` only when you want cached native execution.',
      },
        {
          icon: Braces,
          title: 'Opcode specialization targets shared shapes',
          body: 'Current optimization focuses on general operand shapes and lowering: Int immediates, min/max update, compare-test, nil and zero branches, field keys, ConcatN, Return0/Return1, and fewer temporary-register moves.',
      },
      {
        icon: Code2,
        title: 'AOT for explicit delivery',
        body: '`lk compile exe FILE` takes the native executable path. VM, cached native, and AOT runs share workload checksum validation to keep semantics aligned.',
      },
    ]
  }

  function getTechnicalSections(activeLocale: Locales | undefined): TechnicalSection[] {
    if (activeLocale === 'zh-CN') {
      return [
        {
          kicker: 'Runtime path',
          title: '直接执行默认走 bytecode VM。',
          body: '日常用户运行 `lk FILE` 时使用 VM。cached native execution 是 opt-in 路径，只有设置 `LK_NATIVE_RUN=1` 且 LLVM feature 可用时才会尝试复用 cached native executable。',
          items: [
            'cache key 覆盖源文件内容、当前 `lk` executable 路径/mtime 和 CLI package version。',
            '`LK_NATIVE_CACHE_DIR` 可指定 native cache 目录。',
            '`LK_FORCE_VM=1` 或 `LK_VM_ONLY=1` 仍可显式禁止 native opt-in，用于 benchmark/profile 场景。',
          ],
        },
        {
          kicker: 'Opcode work',
          title: 'Opcode specialization 只覆盖通用 operand shape。',
          body: '当前优化避免 workload-specific fused opcode，优先消除 register materialization、typed fallback 污染和重复 control-flow 解析。',
          items: [
            '`AddIntI`、`MulIntI`、`ModIntI` 覆盖 small-int literal RHS arithmetic；`AddIntI` / `MulIntI` 也覆盖 facts-confirmed `literal + x` / `literal * x` commuted immediate shape。',
            '`AddMulInt` 覆盖 facts-confirmed compound integer multiply-add accumulator shape。',
            '`MinInt` / `MaxInt` 覆盖 facts-confirmed integer min/max update branch。',
            '`BrNil` / `BrNotNil` 覆盖 condition-context nilness branch。',
            '`BrEqZeroInt` / `BrNeZeroInt` 覆盖 facts-confirmed zero-compare false edge。',
            '`BrEqIntI4` / `BrNeIntI4` 覆盖 facts-confirmed `0..15` small-int equality false edge。',
            '`BrModEqZeroIntI4` / `BrModNeZeroIntI4` 覆盖 facts-confirmed `(x % K) == 0` / `(x % K) != 0` divisibility guard，`K` 为 `1..15`。',
            '`TestEqIntI` / `TestNeIntI` 覆盖 facts-confirmed Int 与 i8 literal equality compare-test。',
            '`TestEqIntI2` 覆盖 small-int pair condition；`Move2` 覆盖相邻本地赋值链；`ListPush` 传播 list element kind，typed int-list `GetIndex` 直读 Int backing，facts-confirmed `List<Int> + Int key` 发射 `GetList`；`ModInt` / `ModIntI` 后接同寄存器 zero branch 时跳过下一次 dispatch；nil branch、zero branch、small-int branch/test 与普通 compare-test 后接 `Move + Jmp` 或单条 `Move` 时跳过对应后继 dispatch；`GetFieldK` 后接同寄存器 nilness branch 时直接应用分支并处理 default `Move`；连续 `Move` dispatch 使用 tight next-op check；template literal parts 会进入 loop scalar const cache；`GetFieldK` / `SetFieldK`、`ConcatN`、`Return0` / `Return1` 覆盖通用 field、string concat 和 return shape。',
            'known string key 读取空 `TypedMap::Mixed` 时直接返回 `nil`，减少 sparse map/default lookup 的 `RuntimeMapKey` 构造和 `generic_map_lookup`。',
            'compiler 会把安全的 `let/assign x = default; if-chain { x = value }` 重排为 synthetic final else default，减少分支命中路径上的 default overwrite。',
            '`math.floor(Int-like)`、外部 `map.get` 和普通 indexed access 支持 direct-to-destination lowering，减少 `Move dst, temp`。',
          ],
        },
        {
          kicker: 'Benchmark evidence',
          title: 'VM 是默认路径，native/AOT 是独立性能路径。',
          body: '当前 workload suite 的验证命令默认采用 `RUNS=3 EXTRA_RUNS=5`。默认 bytecode VM 最新复验为 `0.793x` / `0.808x` vs Lua；supported native/AOT shapes 历史约 `0.35x` vs Lua。',
          items: [
            '默认 `lk FILE` 与 `LK_FORCE_VM=1` 都是解释器语义；`LK_NATIVE_RUN=1` 才进入 cached native。',
            '当前 full-suite AOT smoke 仍需补 dynamic-map `GetIndex` native lowering；VM checksum 仍作为默认路径校验。',
            '`AddMulInt` 在 static coverage 中覆盖 `10` 处 compound-add multiply term；当前全 workload instructions 为 `1952`，`AddIntI=58`，`BrModNeZeroIntI4=21`，`GetList=6`，`GetIndex=62`。',
            '`BrNeIntI4` 把 small-int branch chain 收成单条 branch；branch-body peephole 继续减少 nil branch、zero branch、small-int branch/test、field default 与普通 compare-test 的 `Move + Jmp` 和单条 `Move` dispatch。',
            'empty `TypedMap::Mixed` fast path 让 `config_defaults_merge` A/B 从 `1.383x` 改到 `1.204x`；全 workload静态 `Move` 为 `341`。',
          ],
        },
      ]
    }

    return [
      {
        kicker: 'Runtime path',
        title: 'Direct execution uses the bytecode VM by default.',
        body: 'Users can run `lk FILE` and get VM execution. Cached native execution is opt-in: LK only tries a cached native executable when `LK_NATIVE_RUN=1` is set and the LLVM feature is available.',
        items: [
          'The cache key includes source content, current `lk` executable path/mtime, and CLI package version.',
          '`LK_NATIVE_CACHE_DIR` overrides the native cache directory.',
          '`LK_FORCE_VM=1` or `LK_VM_ONLY=1` still disables native opt-in for benchmark/profile runs.',
        ],
      },
      {
        kicker: 'Opcode work',
        title: 'Opcode specialization targets shared operand shapes.',
        body: 'Current optimization avoids workload-specific fused opcodes and focuses on removing register materialization, typed fallback pollution, and repeated control-flow decoding.',
        items: [
          '`AddIntI`, `MulIntI`, and `ModIntI` cover small-int literal RHS arithmetic; `AddIntI` / `MulIntI` also cover facts-confirmed `literal + x` / `literal * x` commuted immediate shapes.',
          '`AddMulInt` covers facts-confirmed compound integer multiply-add accumulator shapes.',
          '`MinInt` / `MaxInt` cover facts-confirmed integer min/max update branches.',
          '`BrNil` / `BrNotNil` cover condition-context nilness branches.',
          '`BrEqZeroInt` / `BrNeZeroInt` cover facts-confirmed zero-compare false edges.',
          '`BrEqIntI4` / `BrNeIntI4` cover facts-confirmed `0..15` small-int equality false edges.',
          '`BrModEqZeroIntI4` / `BrModNeZeroIntI4` cover facts-confirmed `(x % K) == 0` / `(x % K) != 0` divisibility guards for `K` in `1..15`.',
          '`TestEqIntI` / `TestNeIntI` cover facts-confirmed Int and i8 literal equality compare-test shapes.',
          '`TestEqIntI2` covers small-int pair conditions; `Move2` covers adjacent local assignment chains; `ListPush` propagates list element kind, typed int-list `GetIndex` reads go straight to the Int backing, and facts-confirmed `List<Int> + Int key` emits `GetList`; `ModInt` / `ModIntI` followed by a same-register zero branch skips the next dispatch; nil branches, zero branches, small-int branch/test, and ordinary compare-test bodies shaped as `Move + Jmp` or a single `Move` skip the following dispatch; `GetFieldK` followed by a same-register nilness branch applies that branch and default `Move` inside the field-read arm; continuous `Move` dispatch uses a tight next-op check; template literal parts enter the loop scalar const cache; `GetFieldK` / `SetFieldK`, `ConcatN`, and `Return0` / `Return1` cover general field, string concat, and return shapes.',
          'Known string key lookup on empty `TypedMap::Mixed` now returns `nil` directly, reducing `RuntimeMapKey` construction and `generic_map_lookup` for sparse map/default reads.',
          'The compiler sinks safe `let/assign x = default; if-chain { x = value }` shapes into a synthetic final `else` default to reduce default overwrites on taken branch paths.',
          '`math.floor(Int-like)`, external `map.get`, and plain indexed access use direct-to-destination lowering to reduce `Move dst, temp`.',
        ],
      },
      {
        kicker: 'Benchmark evidence',
        title: 'The VM is the default path; native/AOT is a separate performance path.',
        body: 'The current workload suite uses `RUNS=3 EXTRA_RUNS=5` by default. The latest default bytecode VM validation is `0.793x` / `0.808x` vs Lua; supported native/AOT shapes have historically measured about `0.35x` vs Lua.',
        items: [
          'Default `lk FILE` and `LK_FORCE_VM=1` both use interpreter semantics; `LK_NATIVE_RUN=1` enters cached native execution.',
          'Current full-suite AOT smoke still needs dynamic-map `GetIndex` native lowering; VM checksum validation remains the default-path gate.',
          '`AddMulInt` covers `10` compound-add multiply terms in static coverage; current workload instructions are `1952`, with `AddIntI=58`, `BrModNeZeroIntI4=21`, `GetList=6`, and `GetIndex=62`.',
          '`BrNeIntI4` collapses small-int branch chains into one branch; the branch-body peephole further reduces `Move + Jmp` and single-`Move` dispatch in nil branch, zero branch, field-default, small-int branch/test, and ordinary compare-test fallthrough bodies.',
          'The empty `TypedMap::Mixed` fast path moves `config_defaults_merge` from `1.383x` to `1.204x` in A/B validation; static `Move` count across the workload is `341`.',
        ],
      },
    ]
  }

  function getTechnicalCommands(activeLocale: Locales | undefined): string[] {
    if (activeLocale === 'zh-CN') {
      return [
        'lk FILE',
        'LK_NATIVE_RUN=1 lk FILE',
        'lk compile exe FILE',
        'RUN_AOT=0 RUNS=3 EXTRA_RUNS=5 bash bench/run_workload_bench.sh',
      ]
    }

    return [
      'lk FILE',
      'LK_NATIVE_RUN=1 lk FILE',
      'lk compile exe FILE',
      'RUN_AOT=0 RUNS=3 EXTRA_RUNS=5 bash bench/run_workload_bench.sh',
    ]
  }

  $: activeLangDocument = locale === 'zh-CN' ? langZhDocument : langDocument
  $: specSections = parseMarkdown(activeLangDocument)
  $: specNav = buildSpecNav(specSections)

  const lkKeywords = new Set([
    'if',
    'else',
    'while',
    'for',
    'in',
    'match',
    'return',
    'break',
    'continue',
    'select',
    'case',
    'default',
    'let',
    'const',
    'fn',
    'struct',
    'trait',
    'impl',
    'import',
    'from',
    'as',
    'spawn',
    'chan',
    'send',
    'recv',
  ])

  const lkTypes = new Set([
    'Int',
    'Float',
    'String',
    'Bool',
    'Nil',
    'Any',
    'List',
    'Map',
    'Task',
    'Channel',
    'Function',
    'Object',
    'Stream',
    'StreamCursor',
    'Iterator',
    'MutationGuard',
  ])

  const lkConstants = new Set(['true', 'false', 'nil'])

  function escapeHtml(value: string): string {
    return value
      .replace(/&/g, '&amp;')
      .replace(/</g, '&lt;')
      .replace(/>/g, '&gt;')
      .replace(/"/g, '&quot;')
      .replace(/'/g, '&#39;')
  }

  function wrapToken(kind: string, value: string): string {
    return `<span class="lk-token lk-${kind}">${escapeHtml(value)}</span>`
  }

  function isIdentifierStart(char: string | undefined): boolean {
    return !!char && /[A-Za-z_]/.test(char)
  }

  function isIdentifierPart(char: string | undefined): boolean {
    return !!char && /[A-Za-z0-9_-]/.test(char)
  }

  function readQuotedString(source: string, start: number, quote: string): number {
    let index = start + 1
    while (index < source.length) {
      if (source[index] === '\\') {
        index += 2
        continue
      }
      if (source[index] === quote) return index + 1
      index += 1
    }
    return source.length
  }

  function readRawString(source: string, start: number): number | undefined {
    const opener = source.slice(start).match(/^r(#+)?"/)
    if (!opener) return undefined

    const hashes = opener[1] || ''
    const close = `"${hashes}`
    const contentStart = start + opener[0].length
    const closeIndex = source.indexOf(close, contentStart)
    return closeIndex === -1 ? source.length : closeIndex + close.length
  }

  function highlightLkCode(source: string): string {
    let html = ''
    let index = 0
    let inBlockComment = false

    while (index < source.length) {
      if (inBlockComment) {
        const end = source.indexOf('*/', index)
        const nextIndex = end === -1 ? source.length : end + 2
        html += wrapToken('comment', source.slice(index, nextIndex))
        index = nextIndex
        inBlockComment = end === -1
        continue
      }

      const char = source[index]
      const next = source[index + 1]

      if (char === '/' && next === '*') {
        const end = source.indexOf('*/', index + 2)
        const nextIndex = end === -1 ? source.length : end + 2
        html += wrapToken('comment', source.slice(index, nextIndex))
        index = nextIndex
        inBlockComment = end === -1
        continue
      }

      if (char === '/' && next === '/') {
        const end = source.indexOf('\n', index)
        const nextIndex = end === -1 ? source.length : end
        html += wrapToken('comment', source.slice(index, nextIndex))
        index = nextIndex
        continue
      }

      const rawStringEnd = char === 'r' ? readRawString(source, index) : undefined
      if (rawStringEnd !== undefined) {
        html += wrapToken('string', source.slice(index, rawStringEnd))
        index = rawStringEnd
        continue
      }

      if (char === '"' || char === "'" || char === '`') {
        const end = readQuotedString(source, index, char)
        html += wrapToken('string', source.slice(index, end))
        index = end
        continue
      }

      if (/\d/.test(char)) {
        const match = source.slice(index).match(/^\d+(?:\.\d+)?(?:[eE][+-]?\d+)?/)
        if (match) {
          html += wrapToken('number', match[0])
          index += match[0].length
          continue
        }
      }

      if (isIdentifierStart(char)) {
        let end = index + 1
        while (isIdentifierPart(source[end])) end += 1
        const word = source.slice(index, end)
        const rest = source.slice(end).replace(/^\s+/, '')

        if (lkConstants.has(word)) {
          html += wrapToken('constant', word)
        } else if (lkTypes.has(word)) {
          html += wrapToken('type', word)
        } else if (lkKeywords.has(word)) {
          html += wrapToken('keyword', word)
        } else if (rest.startsWith('(')) {
          html += wrapToken('function', word)
        } else {
          html += escapeHtml(word)
        }
        index = end
        continue
      }

      const operator = source
        .slice(index)
        .match(/^(?:\?\?|\?\.|\?\[|=>|->|\.\.=|\.\.|==|!=|<=|>=|\+=|-=|\*=|\/=|%=|&&|\|\||:=|[+\-*/%&|~!?:=<>])/)
      if (operator) {
        html += wrapToken('operator', operator[0])
        index += operator[0].length
        continue
      }

      if (/[\[\]{}().,;]/.test(char)) {
        html += wrapToken('punctuation', char)
        index += 1
        continue
      }

      html += escapeHtml(char)
      index += 1
    }

    return html
  }

  function slugify(value: string): string {
    return value
      .toLowerCase()
      .replace(/`/g, '')
      .replace(/[^a-z0-9]+/g, '-')
      .replace(/^-|-$/g, '')
  }

  function normalizePath(path: string): string {
    return path === '/LANG.md' ? '/spec' : path
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

  function buildSpecNav(sections: SpecSection[]): SpecNavGroup[] {
    const nav: SpecNavGroup[] = []
    let current: SpecNavGroup | undefined = undefined

    for (const section of sections.filter((item) => item.level <= 3)) {
      if (section.level <= 2 || !current) {
        current = { section, children: [] }
        nav.push(current)
        continue
      }

      current.children.push(section)
    }

    return nav
  }

  function parseMarkdown(markdown: string): SpecSection[] {
    const lines = markdown.split('\n')
    const sections: SpecSection[] = []
    let current: SpecSection | undefined = undefined
    let paragraph: string[] = []
    let list: string[] = []
    let code: string[] = []
    let inCode = false

    function ensureSection(): SpecSection {
      if (!current) {
        current = { id: 'overview', level: 2, title: 'Overview', blocks: [] }
        sections.push(current)
      }
      return current
    }

    function flushParagraph(): void {
      if (!paragraph.length) return
      ensureSection().blocks.push({ type: 'paragraph', text: paragraph.join(' ') })
      paragraph = []
    }

    function flushList(): void {
      if (!list.length) return
      ensureSection().blocks.push({ type: 'list', items: list })
      list = []
    }

    function flushTextBlocks(): void {
      flushParagraph()
      flushList()
    }

    for (const line of lines) {
      if (line.startsWith('```')) {
        if (inCode) {
          ensureSection().blocks.push({ type: 'code', text: code.join('\n') })
          code = []
          inCode = false
        } else {
          flushTextBlocks()
          inCode = true
        }
        continue
      }

      if (inCode) {
        code.push(line)
        continue
      }

      const heading = line.match(/^(#{1,6})\s+(.*)$/)
      if (heading) {
        flushTextBlocks()
        const title = heading[2].trim()
        if (heading[1].length === 1 && title === 'Language Overview') {
          continue
        }
        current = {
          id: slugify(title),
          level: heading[1].length,
          title,
          blocks: [],
        }
        sections.push(current)
        continue
      }

      if (!line.trim()) {
        flushTextBlocks()
        continue
      }

      const bullet = line.match(/^\s*-\s+(.*)$/)
      if (bullet) {
        flushParagraph()
        list.push(bullet[1])
        continue
      }

      flushList()
      paragraph.push(line.trim())
    }

    flushTextBlocks()
    return sections
  }

  function navigate(event: MouseEvent, path: string): void {
    event.preventDefault()
    if (typeof window === 'undefined') return
    window.history.pushState({}, '', pathWithLocale(path))
    currentPath = normalizePath(window.location.pathname)
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

  onMount(() => {
    const nextLocale = locale || getInitialLocale()
    applyLocale(nextLocale)
    syncLocaleToUrl(nextLocale)

    const handlePopstate = () => {
      currentPath = normalizePath(window.location.pathname)
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
          <a href={item.href} target="_blank" rel="noreferrer">{$LL.nav[item.label]()}</a>
        {:else}
          <a href={item.href} aria-current={currentPath === item.href ? 'page' : undefined} on:click={(event) => navigate(event, item.href)}>{$LL.nav[item.label]()}</a>
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

  {#if currentPath === '/spec'}
    <section class="spec-layout" aria-label="LK language specification">
      <aside class="spec-toc" aria-label="Specification table of contents">
        <strong>{$LL.spec.toc()}</strong>
        {#each specNav as group}
          <details open>
            <summary>
              <span>{group.section.title}</span>
            </summary>
            <a href={`#${group.section.id}`}>{group.section.title}</a>
            {#each group.children as child}
              <a class="toc-nested" href={`#${child.id}`}>{child.title}</a>
            {/each}
          </details>
        {/each}
      </aside>

      <div class="spec-content">
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
                    <li>{item}</li>
                  {/each}
                </ul>
              {:else if block.type === 'code'}
                <pre><code>{@html highlightLkCode(block.text)}</code></pre>
              {/if}
            {/each}
          </article>
        {/each}
      </div>
    </section>
  {:else if currentPath === '/performance'}
    <section class="technical-page" aria-labelledby="technical-title">
      <div class="technical-hero">
        <p class="eyebrow">{locale === 'zh-CN' ? 'Runtime and Opcode Notes' : 'Runtime and Opcode Notes'}</p>
        <h1 id="technical-title">{locale === 'zh-CN' ? 'LK 性能路径技术说明。' : 'LK performance path, technically.'}</h1>
        <p>
          {locale === 'zh-CN'
            ? '这一页面向想了解实现细节的用户：默认 bytecode VM、可选 cached native execution、LLVM AOT、Opcode specialization 和 benchmark 证据都集中在这里。'
            : 'This page is for users who want implementation detail: default bytecode VM execution, optional cached native execution, LLVM AOT, Opcode specialization, and benchmark evidence live here.'}
        </p>
      </div>

      <div class="technical-command-strip">
        {#each technicalCommands as command}
          <code><Terminal size={16} /> {command}</code>
        {/each}
      </div>

      <div class="technical-sections">
        {#each technicalSections as section, index}
          <article>
            <p class="section-kicker"><span>{String(index + 1).padStart(2, '0')}</span>{section.kicker}</p>
            <h2>{section.title}</h2>
            <p>{section.body}</p>
            <ul>
              {#each section.items as item}
                <li>{item}</li>
              {/each}
            </ul>
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
          <a class="btn btn-primary" href="#start" on:click={(event) => scrollToSection(event, 'start')}>
            <Play size={18} />
            {$LL.hero.primaryAction()}
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

    <section class="performance-section" id="performance" aria-labelledby="performance-title">
      <div class="section-heading">
        <div>
          <div class="section-kicker">
            <span>03</span>
            <p>{locale === 'zh-CN' ? '性能路径' : 'Performance Path'}</p>
          </div>
          <h2 id="performance-title">
            {locale === 'zh-CN' ? '默认 VM，native 显式加速。' : 'VM by default, native by opt-in.'}
          </h2>
        </div>
        <p>
          {locale === 'zh-CN'
            ? 'LK 把用户日常运行、可选 native 加速、显式 AOT 交付和解释器诊断分成清晰路径：默认运行固定在 bytecode VM。'
            : 'LK separates everyday execution, optional native acceleration, explicit AOT delivery, and interpreter diagnostics: the default run path stays on the bytecode VM.'}
        </p>
      </div>
      <div class="metric-strip">
        <Cpu size={18} />
        <span>{performanceMetric}</span>
        <a href="/performance" on:click={(event) => navigate(event, '/performance')}>
          {locale === 'zh-CN' ? '技术细节' : 'Technical details'}
          <ArrowRight size={16} />
        </a>
      </div>
      <div class="performance-grid">
        {#each performanceCards as card}
          <article>
            <svelte:component this={card.icon} size={22} />
            <h3>{card.title}</h3>
            <p>{card.body}</p>
          </article>
        {/each}
      </div>
    </section>

    <section class="stdlib-section" id="stdlib" aria-labelledby="stdlib-title">
      <div class="section-heading compact">
        <div class="section-kicker">
          <span>04</span>
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
            <h3>{example.label === 'Named parameters' ? $LL.examples.namedParameters() : example.label === 'Import forms' ? $LL.examples.importForms() : $LL.examples.collectionPipelines()}</h3>
            </header>
            <pre><code>{@html highlightLkCode(example.code)}</code></pre>
          </article>
        {/each}
      </div>
    </section>

    <section class="start-section" id="start" aria-labelledby="start-title">
      <div>
        <div class="section-kicker">
          <span>05</span>
          <p>{$LL.start.kicker()}</p>
        </div>
        <h2 id="start-title">{$LL.start.title()}</h2>
      </div>
      <div class="command-grid">
        <code><Terminal size={16} /> lk</code>
        <code><Code2 size={16} /> lk check FILE</code>
        <code><Package size={16} /> lk compile FILE</code>
        <code><GitBranch size={16} /> lk pkg tree</code>
        <code><Cable size={16} /> lk compile llvm FILE</code>
      </div>
    </section>
  {/if}

  <footer class="site-footer">
    <span>{$LL.footer.brand()}</span>
    <a href="/" on:click={(event) => navigate(event, '/')}>{$LL.footer.home()}</a>
    <a href="/spec" on:click={(event) => navigate(event, '/spec')}>{$LL.footer.spec()}</a>
  </footer>
</main>
{/if}
