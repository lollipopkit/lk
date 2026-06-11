import type { BaseTranslation } from '../i18n-types'

const en: BaseTranslation = {
  meta: {
    lang: 'en',
    title: 'LK Lang',
    description:
      'LK is a Rust-like language with rich pattern matching, struct/trait system, first-class closures, and a batteries-included standard library.',
  },
  nav: {
    github: 'Github',
    learn: 'Learn',
    stdlib: 'Stdlib',
    languageLabel: 'Language',
  },
  hero: {
    eyebrow: 'Rust-like language',
    title: 'Expressive.\nPractical.\nLightweight.',
    subtitle:
      'Pattern matching, optional chaining, closures, named parameters, struct/trait, derive macros, and concurrency — in one compact language you can embed anywhere.',
    primaryAction: 'Get Started',
    secondaryAction: 'See features',
    previewLabel: 'LK syntax preview',
  },
  feature: {
    kicker: 'Language Features',
    title: 'Designed for clarity and power.',
    groups: {
      destructuring: { title: 'Destructuring' },
      match: { title: 'Pattern Matching' },
      optionalChaining: { title: 'Optional Chaining' },
      templateStrings: { title: 'Template Strings' },
      ranges: { title: 'Range Literals' },
      closures: { title: 'First-class Closures' },
      namedParams: { title: 'Named Parameters' },
      traits: { title: 'Structs & Traits' },
      derive: { title: 'Derive Macros' },
      concurrency: { title: 'Concurrency' },
    },
  },
  showcase: {
    kicker: 'In Practice',
    title: 'Real code, real clarity.',
  },
  playground: {
    ariaLabel: 'LK playground',
    editorAriaLabel: 'LK source editor',
    sourceAriaLabel: 'LK source code',
    outputAriaLabel: 'Run output',
    selectSample: 'Select LK sample',
    resetSource: 'Reset source',
    copyOutput: 'Copy output',
    run: 'Run',
    running: 'Running',
    loadingWasm: 'Loading wasm',
    ready: 'Ready',
    unavailable: 'Unavailable',
    completed: 'Completed',
    failed: 'Failed',
    emptyMessage: 'Select a sample or edit the source, then run it in the wasm sandbox.',
    examples: {
      patternMatching: 'Pattern matching',
      structTrait: 'Structs & traits',
      namedParams: 'Named params',
      ranges: 'Ranges',
      templateStrings: 'Template strings',
      errorHandling: 'Error handling',
      closures: 'Closures',
      configParser: 'Config parser',
      sortSearch: 'Sort & search',
      listIterSugar: 'List / iter interop',
      listOps: 'List operations',
      jsonProcess: 'JSON processing',
      macros: 'Macros',
      custom: 'Custom',
    },
  },
  runtime: {
    kicker: 'Runtime & Tooling',
    title: 'From REPL to native builds.',
    rows: {
      valueModel: 'Value model',
      execution: 'Execution',
      imports: 'Imports',
      concurrency: 'Concurrency',
    },
  },
  stdlib: {
    kicker: 'Stdlib',
    title: 'Batteries included.',
    eyebrow: 'Standard Library',
    subtitle:
      'Module-by-module reference with function tables and runnable examples for every stdlib module.',
    toc: 'On this page',
  },
  start: {
    kicker: 'Get Started',
    title: 'Up and running in seconds.',
  },
  learn: {
    eyebrow: 'Tutorial',
    title: 'Learn LK step by step.',
    subtitle:
      'A progressive tutorial that takes you from your first LK program through pattern matching, structs, traits, modules, and macros.',
    toc: 'On this page',
  },
  footer: {
    brand: 'LK Lang',
    home: 'Home',
    learn: 'Learn',
    stdlib: 'Stdlib',
  },
}

export default en
