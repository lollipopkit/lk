import type { Translation } from '../i18n-types'

const zhCN: Translation = {
  meta: {
    lang: 'zh-CN',
    title: 'LK Lang',
    description: 'LK 是 Rust 编写的 Rust 风格高性能轻量脚本语言，支持 bytecode VM、LLVM IR 和可执行文件路径，包含丰富语法糖',
  },
  nav: {
    spec: '规范',
    github: 'Github',
    languageLabel: '语言',
  },
  hero: {
    eyebrow: '用 Rust 编写的 Rust 风格脚本语言',
    title: '高性能 轻量 现代 脚本语言。',
    subtitle:
      'LK 提供清晰的语法、确定性的 VM 执行、结构化模式匹配和实用标准库，支持模块产物、LLVM IR 和可执行文件路径。',
    primaryAction: '开始',
    secondaryAction: '查看特性',
    previewLabel: '语法预览',
  },
  feature: {
    kicker: '概览',
    title: '现代 紧凑的语法 清晰的行为。',
    subtitle:
      '`LANG.md` 描述了 LK 的语言行为：外部输入必须显式读取，关键字保留，支持 Rust 风格 raw string，普通字符串支持插值。',
    groups: {
      expression: {
        title: '现代表达式核心',
        body:
          '模板字符串、空值合并、右结合三元表达式、可选链、范围字面量、位运算和一等闭包都在同一套紧凑表达式语法中。',
      },
      collections: {
        title: '内建 List 和 Map',
        body:
          '异构集合支持负索引、切片、展开、点访问、复合赋值、列表差集、Map 合并和 meta-method 分发。',
      },
      patterns: {
        title: '到处可用的模式匹配',
        body:
          'match、if let、while let、let 解构和 for 循环解构覆盖字面量、范围、Map、List、rest 绑定、or-pattern 和 guard。',
      },
      traits: {
        title: 'Struct、Trait 和方法',
        body:
          '可以定义记录、实现 trait，并通过直接属性或运行时 meta-method 分发调用方法；display 方法会自动参与格式化。',
      },
    },
  },
  runtime: {
    kicker: '运行时与工具链',
    title: '从脚本到 VM 执行、诊断和 true-native LLVM。',
    subtitle:
      'LK 可以运行 REPL、执行 `.lk` 文件、不执行只做类型检查，并把已支持的 shape 降到 true-native LLVM IR，不再保留 artifact shell 或 host launcher fallback。',
    rows: {
      valueModel: '值模型',
      execution: '执行',
      imports: '导入',
      concurrency: '并发',
    },
  },
  stdlib: {
    kicker: '标准库',
    title: '实用模块是语言设计的一部分。',
  },
  examples: {
    title: '示例与当前语言参考保持一致。',
    subtitle: '这些片段聚焦 `LANG.md` 中记录的行为：命名参数、安全相对导入和集合式高阶工具。',
    namedParameters: '命名参数',
    importForms: '导入形式',
    collectionPipelines: '集合管线',
  },
  start: {
    kicker: 'CLI',
    title: '从终端使用，或嵌入核心运行时。',
  },
  spec: {
    eyebrow: '语言参考',
    title: '由 LANG.md 渲染的 LK 语言规范。',
    subtitle:
      '该页面直接以仓库中的 `LANG.md` 为来源，用网页布局展示 parser、evaluator、statement、type、import、package、CLI 和 runtime notes。',
    toc: '本页目录',
  },
  footer: {
    brand: 'LK Lang',
    home: '首页',
    spec: '规范',
  },
}

export default zhCN
