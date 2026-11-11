import type { EngineInterface, Register } from 'claude-code'

/** The slash command that opens a diagram in a pane. */
const COMMAND = 'flowcli-open'
/** Where flowcli's Herdr plugin logs events: next to the diagram. */
const EVENTS_SUFFIX = '.events.jsonl'
/** How far back to look for the last selection. Selections are frequent
 *  and lines are short, so the tail is enough and stays cheap on a log
 *  that has grown for weeks. */
const TAIL_LINES = '200'
const DIAGRAM = /\.(mmd|mermaid)$/i
/** `--env FLOWCLI_FILE=<path>` in a `herdr plugin pane open` command. */
const PANE_FILE = /FLOWCLI_FILE=(?:"([^"]+)"|'([^']+)'|(\S+))/

type Selection = {
  ts: number
  /** What is selected, in words; undefined when nothing is. */
  what?: string
}

const quoted = (text: unknown): string => JSON.stringify(String(text ?? ''))

/** One event line as the phrase the model reads, or undefined for an
 *  event that is not a selection. */
const describe = (ev: Record<string, unknown>): string | undefined => {
  if (ev.event !== 'select') {
    return undefined
  }
  if (ev.kind === 'node') {
    return `node ${quoted(ev.id)} (label ${quoted(ev.label)})`
  }
  if (ev.kind === 'edge') {
    const label = ev.label == null ? '' : ` (label ${quoted(ev.label)})`

    return `edge from ${quoted(ev.from)} to ${quoted(ev.to)}${label}`
  }
  if (ev.kind === 'cluster') {
    return `group ${quoted(ev.id)} (title ${quoted(ev.title)})`
  }

  return undefined
}

/**
 * The current selection according to the last lines of an events log.
 * `open` and `quit` end the search: a selection from before flowcli was
 * closed or restarted is no longer on screen.
 */
export const selection = (log: string): Selection | undefined => {
  const lines = log.split('\n')
  for (let i = lines.length - 1; i >= 0; i -= 1) {
    const line = lines[i]?.trim()
    if (!line) {
      continue
    }
    let ev: Record<string, unknown>
    try {
      ev = JSON.parse(line)
    } catch {
      // A line cut by `tail`, or not ours.
      continue
    }
    const ts = typeof ev.ts === 'number' ? ev.ts : 0
    if (ev.event === 'open' || ev.event === 'quit' || ev.event === 'clear') {
      return { ts }
    }
    const what = describe(ev)
    if (what !== undefined) {
      return { ts, what }
    }
  }

  return undefined
}

const absolute = async ($: EngineInterface, path: string) =>
  path.startsWith('/') ? path : `${await $.session.cwd()}/${path}`

/** The selection in `file`'s events log, undefined when there is no log. */
const current = async ($: EngineInterface, file: string) => {
  const ran = await $.process
    .run(['tail', '-n', TAIL_LINES, file + EVENTS_SUFFIX])
    .catch(() => undefined)

  return ran?.exitCode === 0 ? selection(ran.stdout) : undefined
}

export const register: Register = on => {
  /** The diagram this session is working on, absolute. */
  let diagram: string | undefined
  /** The selection the model was last told about; undefined when it was
   *  told nothing, or that nothing is selected. */
  let told: string | undefined

  on('session.start', async ($, e, next) => {
    await $.command.register({
      name: COMMAND,
      description: 'Open a diagram in a flowcli pane next to this one',
      argumentHint: '[file.mmd]',
    })

    return next(e)
  })

  // The diagram is whichever one the agent last wrote or opened. These
  // hooks only watch: when one fails, the call goes on untouched.
  on('tool.call', { tool: 'Write' }, async ($, e, next) => {
    if (DIAGRAM.test(e.file_path)) {
      diagram = await absolute($, e.file_path)
    }

    return next(e)
  }).catch(($, e, next) => next(e))

  on('tool.call', { tool: 'Edit' }, async ($, e, next) => {
    if (DIAGRAM.test(e.file_path)) {
      diagram = await absolute($, e.file_path)
    }

    return next(e)
  }).catch(($, e, next) => next(e))

  on('tool.call', { tool: 'Bash' }, async ($, e, next) => {
    const found = PANE_FILE.exec(e.command)
    const file = found?.[1] ?? found?.[2] ?? found?.[3]
    if (file !== undefined) {
      diagram = await absolute($, file)
    }

    return next(e)
  }).catch(($, e, next) => next(e))

  // Tell the model what is selected, once per change, so "explain this"
  // works without it reading the log first.
  on('prompt.submit', async ($, e, next) => {
    if (e.isMeta || diagram === undefined) {
      return next(e)
    }
    const now = await current($, diagram)
    const key = now?.what === undefined ? undefined : `${diagram}@${now.ts}`
    if (key === told) {
      return next(e)
    }
    told = key
    const note =
      now?.what === undefined
        ? `flowcli: nothing is selected in ${diagram} any more.`
        : `flowcli: in the diagram ${diagram} the user has selected ${now.what}. ` +
          'This is background for when they refer to the diagram ("this", ' +
          '"the one I clicked"). It is not a request: do not act on it unless ' +
          'the prompt refers to it.'

    return next({ ...e, context: [...(e.context ?? []), note] })
  }).catch(($, e, next) => next(e))

  on('command.run', { command: COMMAND }, async ($, e) => {
    const arg = e.args.trim().replace(/^(["'])(.*)\1$/, '$2')
    const file = arg === '' ? diagram : await absolute($, arg)
    if (file === undefined) {
      return { text: `Usage: /${COMMAND} <file.mmd>` }
    }
    if (!(await $.fs.exists(file))) {
      return { text: `${file} does not exist.` }
    }
    diagram = file
    const events = file + EVENTS_SUFFIX
    const byHand =
      `Run it in another terminal and selections are still picked up:\n` +
      `  flowcli ${quoted(file)} --watch --events ${quoted(events)}`
    if ((await $.env.get('HERDR_ENV')) === undefined) {
      return { text: `this session is not in a Herdr pane. ${byHand}` }
    }
    const ran = await $.process
      .run([
        'herdr', 'plugin', 'pane', 'open',
        '--plugin', 'flowcli', '--entrypoint', 'view',
        '--placement', 'split', '--direction', 'right', '--no-focus',
        '--env', `FLOWCLI_FILE=${file}`,
      ])
      .catch((error: unknown) => ({ exitCode: -1, stdout: '', stderr: String(error) }))
    if (ran.exitCode !== 0) {
      const why = (ran.stderr || ran.stdout).trim()

      return { text: `Herdr could not open the pane: ${why}\n${byHand}` }
    }

    return { text: `showing ${file}. What you select there is passed to Claude with your next prompt.` }
  }).catch(() => ({ text: 'The command failed; see `claude --debug`.' }))
}
