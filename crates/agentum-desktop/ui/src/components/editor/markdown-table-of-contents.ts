import remarkFrontmatter from 'remark-frontmatter'
import remarkGfm from 'remark-gfm'
import remarkParse from 'remark-parse'
import { unified } from 'unified'
import { MarkdownHeadingSlugger } from './markdown-heading-slug'

type MarkdownTocLevel = 1 | 2 | 3

export type MarkdownTocItem = {
  children: MarkdownTocItem[]
  id: string
  level: MarkdownTocLevel
  title: string
}

const htmlEntitiesForToc = new Map([
  ['amp', '&'],
  ['apos', "'"],
  ['gt', '>'],
  ['lt', '<'],
  ['nbsp', ' '],
  ['quot', '"']
])

function isMarkdownTocLevel(value: number): value is MarkdownTocLevel {
  return value === 1 || value === 2 || value === 3
}

// Scoped local fork of the tiny entities@6.0.1 surface Agentum used here.
// Why: TOC labels only need common/numeric entity decoding before inline
// Markdown stripping, not the full entity database.
function decodeTocHtmlEntities(text: string): string {
  return text.replace(/&(#x[0-9a-f]+|#\d+|[a-z][a-z0-9]+);/gi, (match, entity: string) => {
    const normalized = entity.toLowerCase()
    if (normalized.startsWith('#x')) {
      const codePoint = Number.parseInt(normalized.slice(2), 16)
      return Number.isInteger(codePoint) && codePoint >= 0 && codePoint <= 0x10ffff
        ? String.fromCodePoint(codePoint)
        : match
    }
    if (normalized.startsWith('#')) {
      const codePoint = Number.parseInt(normalized.slice(1), 10)
      return Number.isInteger(codePoint) && codePoint >= 0 && codePoint <= 0x10ffff
        ? String.fromCodePoint(codePoint)
        : match
    }
    return htmlEntitiesForToc.get(normalized) ?? match
  })
}

export function stripInlineMarkdownForToc(text: string): string {
  const tree = parseMarkdownForToc(text)
  return normalizeTocPlainText(markdownAstNodeToText(tree))
}

function normalizeTocPlainText(text: string): string {
  return decodeTocHtmlEntities(text)
    .replace(/\[\[[^|\]]+\|([^\]]+)\]\]/g, '$1')
    .replace(/\[\[([^\]]+)\]\]/g, '$1')
    // Removing each delimiter character is stable under repetition: unlike a
    // whole-tag regex, nested input cannot expose a new `<script>` token.
    .replace(/[<>]/g, '')
    .replace(/\s+/g, ' ')
    .trim()
}

function nearestParent(stack: MarkdownTocItem[], level: MarkdownTocLevel): MarkdownTocItem {
  for (let index = stack.length - 1; index >= 0; index -= 1) {
    const item = stack.at(index)
    if (item && item.level < level) {
      return item
    }
  }
  return stack[0]
}

function appendTocItem(stack: MarkdownTocItem[], item: MarkdownTocItem): void {
  nearestParent(stack, item.level).children.push(item)
  Reflect.set(stack, item.level, item)
  stack.length = item.level + 1
}

type MarkdownAstNode = {
  alt?: string
  children?: MarkdownAstNode[]
  depth?: number
  type?: string
  value?: string
}

// Scoped local fork of mdast-util-to-string@4.0.0 for heading nodes.
// Why: TOC generation only needs text/alt/child concatenation from parsed
// Markdown headings, so a local walker keeps the dependency boundary smaller.
function markdownAstNodeToText(node: MarkdownAstNode): string {
  // Raw HTML is represented as an explicit mdast node. Ignore that node
  // instead of trying to sanitize markup with repeated string replacement;
  // nested/malformed tags can otherwise synthesize a new tag after removal.
  if (node.type === 'html') {
    return ''
  }
  if (typeof node.value === 'string') {
    return node.value
  }
  if (typeof node.alt === 'string') {
    return node.alt
  }
  return (node.children ?? []).map(markdownAstNodeToText).join('')
}

function parseMarkdownForToc(markdown: string): MarkdownAstNode {
  return unified()
    .use(remarkParse)
    .use(remarkGfm)
    .use(remarkFrontmatter, ['yaml', 'toml'])
    .parse(markdown) as MarkdownAstNode
}

export function buildMarkdownTableOfContents(markdown: string): MarkdownTocItem[] {
  const slugger = new MarkdownHeadingSlugger()
  const root: MarkdownTocItem = { id: 'toc-root', level: 1, title: '', children: [] }
  const stack: MarkdownTocItem[] = [root]

  // Why: parsing Markdown keeps the TOC aligned with rendered setext/GFM/entity
  // headings without carrying separate mdast stringifier/entity packages.
  const tree = parseMarkdownForToc(markdown)

  function visit(node: MarkdownAstNode): void {
    if (
      node.type === 'heading' &&
      typeof node.depth === 'number' &&
      isMarkdownTocLevel(node.depth)
    ) {
      const title = normalizeTocPlainText(markdownAstNodeToText(node))
      if (title) {
        appendTocItem(stack, {
          children: [],
          id: slugger.slug(title),
          level: node.depth,
          title
        })
      }
    }
    for (const child of node.children ?? []) {
      visit(child)
    }
  }

  visit(tree)

  return root.children
}
