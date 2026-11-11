import { expect, test } from 'claude-code/testing'

import type { PromptSubmitInput } from 'claude-code'

import { selection } from '../hooks/register'

const prompt = (text: string): PromptSubmitInput => ({ text, wait: false, origin: { kind: 'composer' } })
const line = (fields: object) => JSON.stringify({ file: '/d/x.mmd', ...fields })
const AUTH = line({ event: 'select', ts: 5, kind: 'node', id: 'Auth', label: 'Auth Service', via: 'click' })

test('the last select wins, and clear, open and quit end it', () => {
  expect(selection(AUTH)?.what).toBe('node "Auth" (label "Auth Service")')
  expect(selection(`${AUTH}\n${line({ event: 'drag_end', ts: 6, id: 'Auth', x: 1, y: 2 })}\n`)?.what).toContain('Auth')
  for (const event of ['clear', 'open', 'quit']) {
    expect(selection(`${AUTH}\n${line({ event, ts: 7 })}`)).toEqual({ ts: 7 })
  }
  const edge = line({ event: 'select', ts: 8, kind: 'edge', from: 'A', to: 'B', label: null })
  expect(selection(edge)?.what).toBe('edge from "A" to "B"')
  // A line `tail` cut in half is skipped, not fatal.
  expect(selection(`{"event":"sel\n${AUTH}`)?.ts).toBe(5)
  expect(selection('')).toBeUndefined()
})

test('a selection reaches the model once, with the next prompt', async ($, on) => {
  let log = AUTH
  const seen: (readonly string[] | undefined)[] = []
  on('command.register', () => ({ value: { command: 'flowcli-open' } }))
  on('session.start', (_, e) => ({ cwd: e.cwd }))
  on('session.cwd', () => ({ value: '/d' }))
  on('tool.call', () => ({ result: {} }))
  on('process.run', () => ({ value: { exitCode: 0, stdout: log, stderr: '', isStdoutTruncated: false, isStderrTruncated: false },
   }))
  on('prompt.submit', (_, e) => {
    seen.push(e.context)

    return { text: e.text }
  })

  await $.session.start({ cwd: '/d', surface: 'terminal', isInteractive: true })
  await $.prompt.submit(prompt('before any diagram'))
  await $.tool.call({ tool: 'Write', file_path: 'x.mmd', content: 'flowchart LR\nA --> B\n' })
  await $.prompt.submit(prompt('explain this'))
  await $.prompt.submit(prompt('and again'))
  log = `${AUTH}\n${line({ event: 'clear', ts: 9 })}`
  await $.prompt.submit(prompt('now?'))

  expect(seen[0]).toBeUndefined()
  expect(seen[1]?.[0]).toContain('/d/x.mmd the user has selected node "Auth"')
  expect(seen[2]).toBeUndefined()
  expect(seen[3]?.[0]).toContain('nothing is selected')
})
