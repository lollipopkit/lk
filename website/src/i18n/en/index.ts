import type { BaseTranslation } from '../i18n-types'

const en: BaseTranslation = {
  meta: {
    lang: 'en',
    title: 'LK Lang',
    description:
      'LK is a Rust-like scripting language with rich pattern matching, package uses, practical CLI workflows, and a batteries-included standard library.',
  },
  nav: {
    try: 'Try',
    spec: 'Spec',
    github: 'Github',
    languageLabel: 'Language',
  },
  hero: {
    eyebrow: 'Rust-like scripting language written in Rust',
    title: 'Lightweight, modern and efficient',
    subtitle:
      'LK gives you clear syntax, structured pattern matching, practical CLI workflows, and a useful standard library for embedding logic in applications and writing automation scripts.',
    primaryAction: 'Start',
    secondaryAction: 'Read features',
    previewLabel: 'LK syntax preview',
  },
  feature: {
    kicker: 'Language Surface',
    title: 'Dense syntax without hidden context.',
    subtitle:
      '`LANG.md` describes a language that keeps external input explicit, reserves keywords, supports Rust-style raw strings, and treats normal quoted strings as interpolation-ready.',
    groups: {
      expression: {
        title: 'Modern expression core',
        body:
          'Template strings, nullish coalescing, right-associative ternaries, optional chaining, range literals, bitwise operators, and first-class closures live in the same compact expression grammar.',
      },
      collections: {
        title: 'Lists and maps built in',
        body:
          'Heterogeneous collections support negative indexing, slicing, spread, dot access, compound assignment, list subtraction, map merges, and meta-method dispatch.',
      },
      patterns: {
        title: 'Pattern matching everywhere',
        body:
          'Match arms, if let, while let, let destructuring, and for-loop destructuring cover literals, ranges, maps, lists, rest bindings, or-patterns, and guards.',
      },
      traits: {
        title: 'Structs, traits, and methods',
        body:
          'Define records, implement traits, call methods through direct properties or runtime meta-method dispatch, and let display methods format values automatically.',
      },
    },
  },
  runtime: {
    kicker: 'Runtime and Tooling',
    title: 'From scripts to type checks, native builds, and package tools.',
    subtitle:
      'LK can run a REPL, execute `.lk` files, type-check without executing, emit native executables or bytecode artifacts, and manage package workspaces.',
    rows: {
      valueModel: 'Value model',
      execution: 'Execution',
      imports: 'Uses',
      concurrency: 'Concurrency',
    },
  },
  stdlib: {
    kicker: 'Stdlib',
    title: 'Useful modules are part of the language story.',
  },
  examples: {
    title: 'Examples that mirror the current language reference.',
    subtitle:
      'These snippets focus on the behavior documented in `LANG.md`: named parameters, relative-safe module uses, and collection-oriented higher-order helpers.',
    namedParameters: 'Named parameters',
    importForms: 'Use forms',
    collectionPipelines: 'Collection pipelines',
  },
  start: {
    kicker: 'CLI',
    title: 'Use it from the terminal or embed the core runtime.',
  },
  spec: {
    eyebrow: 'Language Reference',
    title: 'LK language specification, rendered from LANG.md.',
    subtitle:
      'This page uses the repository `LANG.md` as its source and presents the parser, evaluator, statement, type, module use, package, CLI, and runtime notes in a web-native layout.',
    toc: 'On this page',
  },
  footer: {
    brand: 'LK Lang',
    home: 'Home',
    spec: 'Spec',
  },
}

export default en
