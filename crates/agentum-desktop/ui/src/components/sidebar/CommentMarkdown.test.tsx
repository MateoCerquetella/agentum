import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
// CommentMarkdown is a lazy wrapper (defers the markdown engine off the startup
// chunk); render assertions target the synchronous impl, and the pure autolink
// plugin is tested via its react-markdown-free module.
import CommentMarkdown from './CommentMarkdownImpl'
import { remarkGitHubReferences } from './comment-github-references'

describe('CommentMarkdown', () => {
  it('autolinks same-repo GitHub issue references when repo context is provided', () => {
    const markup = renderToStaticMarkup(
      <CommentMarkdown
        variant="document"
        githubRepo={{ owner: 'mateocerquetella', repo: 'agentum' }}
        content="Automated fix-PR from pr-bug-scan for parent **#2316**."
      />
    )

    expect(markup).toContain('href="https://github.com/mateocerquetella/agentum/issues/2316"')
    expect(markup).toContain('<strong><a')
  })

  it('autolinks cross-repo GitHub issue references', () => {
    const markup = renderToStaticMarkup(
      <CommentMarkdown
        variant="document"
        githubRepo={{ owner: 'mateocerquetella', repo: 'agentum' }}
        content="See another-org/other-repo#42."
      />
    )

    expect(markup).toContain('href="https://github.com/another-org/other-repo/issues/42"')
  })

  it('does not autolink GitHub issue references inside existing links or code', () => {
    const markup = renderToStaticMarkup(
      <CommentMarkdown
        variant="document"
        githubRepo={{ owner: 'mateocerquetella', repo: 'agentum' }}
        content="[`#2316`](https://example.com/already-linked) and `#2317`"
      />
    )

    expect(markup).toContain('href="https://example.com/already-linked"')
    expect(markup).not.toContain('href="https://github.com/mateocerquetella/agentum/issues/2316"')
    expect(markup).not.toContain('href="https://github.com/mateocerquetella/agentum/issues/2317"')
  })

  it('keeps remote compact markdown images as links', () => {
    const markup = renderToStaticMarkup(
      <CommentMarkdown content="See this: ![Image #1](https://example.com/screenshot.png)" />
    )

    expect(markup).not.toContain('<img')
    expect(markup).toContain('href="https://example.com/screenshot.png"')
    expect(markup).toContain('>Image #1</a>')
  })

  it('renders trusted compact markdown images inline', () => {
    const markup = renderToStaticMarkup(
      <CommentMarkdown content="See this: ![Image #1](data:image/png;base64,abc123)" />
    )

    expect(markup).toContain('<img')
    expect(markup).toContain('alt="Image #1"')
    expect(markup).toContain('src="data:image/png;base64,abc123"')
  })

  it('autolinks very large generated GitHub reference comments', () => {
    const referenceCount = 130_000
    const tree = {
      type: 'root',
      children: [
        {
          type: 'paragraph',
          children: [
            {
              type: 'text',
              value: Array.from({ length: referenceCount }, (_, index) => `#${index + 1}`).join(' ')
            }
          ]
        }
      ]
    }

    const transform = remarkGitHubReferences({ owner: 'mateocerquetella', repo: 'agentum' })()

    expect(() => transform(tree)).not.toThrow()
    expect(tree.children[0]?.children).toHaveLength(referenceCount * 2 - 1)
    expect(tree.children[0]?.children[0]).toMatchObject({
      type: 'link',
      url: 'https://github.com/mateocerquetella/agentum/issues/1'
    })
  })

  it('contains long PR body markdown inside its available width', () => {
    const markup = renderToStaticMarkup(
      <CommentMarkdown
        variant="document"
        content={[
          '`src/main/hooks.ts:289 getEffectiveHookScript with policy=shared-only returns yamlScript?.trim() only; localScript is ignored`',
          '',
          '```',
          'const veryLongLine = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";',
          '```'
        ].join('\n')}
      />
    )

    expect(markup).toContain('min-w-0')
    expect(markup).toContain('max-w-full')
    expect(markup).toContain('[overflow-wrap:anywhere]')
    expect(markup).toContain('overflow-x-auto')
  })
})
