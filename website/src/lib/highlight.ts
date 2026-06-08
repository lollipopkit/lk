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
  'use',
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

export function escapeHtml(value: string): string {
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

export function highlightLkCode(source: string): string {
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
