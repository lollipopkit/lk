export type DocBlock =
  | { type: 'paragraph'; text: string }
  | { type: 'list'; items: string[] }
  | { type: 'code'; text: string; lang?: string }
  | { type: 'table'; headers: string[]; rows: string[][] }

export type DocSection = {
  id: string
  level: number
  title: string
  blocks: DocBlock[]
}

export type DocNavGroup = {
  section: DocSection
  children: DocSection[]
}

export function slugify(value: string): string {
  return value
    .toLowerCase()
    .replace(/`/g, '')
    .replace(/[^\p{Letter}\p{Number}]+/gu, '-')
    .replace(/^-|-$/g, '')
}

export function uniqueSlug(title: string, seen: Map<string, number>): string {
  const base = slugify(title) || 'section'
  const count = seen.get(base) || 0
  seen.set(base, count + 1)
  return count === 0 ? base : `${base}-${count + 1}`
}

export function parseDocMarkdown(markdown: string): DocSection[] {
  const lines = markdown.split('\n')
  const sections: DocSection[] = []
  const seenIds = new Map<string, number>()
  let current: DocSection | undefined = undefined
  let paragraph: string[] = []
  let list: string[] = []
  let code: string[] = []
  let codeLang: string | undefined = undefined
  let inCode = false
  let tableHeaders: string[] | undefined
  let tableRows: string[][] = []

  function ensureSection(): DocSection {
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

  function flushTable(): void {
    if (!tableHeaders) return
    ensureSection().blocks.push({ type: 'table', headers: tableHeaders, rows: tableRows })
    tableHeaders = undefined
    tableRows = []
  }

  function flushTextBlocks(): void {
    flushParagraph()
    flushList()
    flushTable()
  }

  for (const line of lines) {
    if (line.startsWith('```')) {
      if (inCode) {
        ensureSection().blocks.push({ type: 'code', text: code.join('\n'), lang: codeLang })
        code = []
        codeLang = undefined
        inCode = false
      } else {
        flushTextBlocks()
        inCode = true
        codeLang = line.slice(3).trim() || undefined
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
        id: uniqueSlug(title, seenIds),
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

    // Table rows
    const pipeMatch = line.match(/^\|(.+)\|$/)
    if (pipeMatch) {
      flushParagraph()
      flushList()
      const cells = pipeMatch[1].split('|').map(c => c.trim())
      // Check separator row
      if (cells.every(c => /^[-:]+$/.test(c))) {
        continue
      }
      if (!tableHeaders) {
        tableHeaders = cells
      } else {
        tableRows.push(cells)
      }
      continue
    }

    // If we were in a table but next line isn't a table, flush
    if (tableHeaders) {
      flushTable()
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

export function buildDocNav(sections: DocSection[], maxLevel: number = 3): DocNavGroup[] {
  const nav: DocNavGroup[] = []
  let current: DocNavGroup | undefined = undefined

  for (const section of sections.filter(item => item.level <= maxLevel)) {
    if (section.level <= 2 || !current) {
      current = { section, children: [] }
      nav.push(current)
      continue
    }

    current.children.push(section)
  }

  return nav
}
