#!/usr/bin/env tsx
/**
 * A todo-list CLI built with incur (TypeScript).
 *
 * Same app as the Rust example for side-by-side comparison.
 *
 * Usage:
 *   npx tsx examples/todoapp.ts --help
 *   npx tsx examples/todoapp.ts add "Buy groceries" --priority high
 *   npx tsx examples/todoapp.ts list
 *   npx tsx examples/todoapp.ts list --status done
 *   npx tsx examples/todoapp.ts get 1
 *   npx tsx examples/todoapp.ts complete 1
 *   npx tsx examples/todoapp.ts stats
 *   npx tsx examples/todoapp.ts stream
 *   npx tsx examples/todoapp.ts list --json
 *   npx tsx examples/todoapp.ts list --format yaml
 *   npx tsx examples/todoapp.ts --version
 */

import { Cli, z, middleware as createMiddleware } from '../src/index.js'

// ---------------------------------------------------------------------------
// Schemas
// ---------------------------------------------------------------------------

const Priority = z.enum(['low', 'medium', 'high'])

const addArgs = z.object({
  title: z.string().describe('The todo title'),
})

const addOptions = z.object({
  priority: Priority.default('medium').describe('Priority level'),
})

const listOptions = z.object({
  status: z.enum(['all', 'pending', 'done']).default('all').describe('Filter by status'),
  limit: z.number().default(50).describe('Maximum number of results'),
})

const getArgs = z.object({
  id: z.coerce.number().describe('The todo ID'),
})

const completeArgs = z.object({
  id: z.coerce.number().describe('The todo ID to complete'),
})

// ---------------------------------------------------------------------------
// Data
// ---------------------------------------------------------------------------

const todos = [
  { id: 1, title: 'Buy groceries', priority: 'high', status: 'pending' },
  { id: 2, title: 'Write docs', priority: 'medium', status: 'done' },
  { id: 3, title: 'Fix bug #123', priority: 'high', status: 'pending' },
  { id: 4, title: 'Review PR', priority: 'low', status: 'done' },
  { id: 5, title: 'Deploy v2', priority: 'medium', status: 'pending' },
]

// ---------------------------------------------------------------------------
// Middleware
// ---------------------------------------------------------------------------

const loggingMiddleware = createMiddleware(async (c, next) => {
  if (!c.agent) {
    process.stderr.write(`[todoapp] running \`${c.command}\`\n`)
  }
  await next()
})

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

const cli = Cli.create('todoapp', {
  description: 'A simple todo list manager',
  version: '0.1.0',
})
  .use(loggingMiddleware)
  .command('add', {
    description: 'Add a new todo item',
    args: addArgs,
    options: addOptions,
    alias: { priority: 'p' },
    examples: [
      { command: 'add "Buy groceries"', description: 'Add with default priority' },
      { command: 'add "Fix bug" --priority high', description: 'Add with high priority' },
      { command: 'add "Read book" -p low', description: 'Add with short alias' },
    ],
    run({ args, options, ok }) {
      return ok(
        {
          id: 42,
          title: args.title,
          priority: options.priority,
          status: 'pending',
        },
        {
          cta: {
            description: 'Next steps:',
            commands: ['list', { command: 'get 42', description: 'View the new todo' }],
          },
        },
      )
    },
  })
  .command('list', {
    description: 'List todo items',
    options: listOptions,
    alias: { status: 's', limit: 'n' },
    examples: [
      { command: 'list', description: 'List all todos' },
      { command: 'list --status pending', description: 'List only pending' },
      { command: 'list --json', description: 'Output as JSON' },
    ],
    run({ options }) {
      const filtered =
        options.status === 'all' ? todos : todos.filter((t) => t.status === options.status)
      return filtered
    },
  })
  .command('get', {
    description: 'Get a todo by ID',
    args: getArgs,
    examples: [{ command: 'get 1', description: 'Get todo #1' }],
    run({ args, error }) {
      const id = args.id
      if (id === 0 || id > 5) {
        return error({
          code: 'NOT_FOUND',
          message: `Todo #${id} not found`,
          cta: {
            description: 'Try listing all todos:',
            commands: ['list'],
          },
        })
      }
      return {
        id,
        title: `Todo #${id}`,
        priority: 'medium',
        status: 'pending',
        created_at: '2026-03-21T12:00:00Z',
      }
    },
  })
  .command('complete', {
    description: 'Mark a todo as done',
    args: completeArgs,
    examples: [{ command: 'complete 1', description: 'Complete todo #1' }],
    run({ args }) {
      return {
        id: args.id,
        status: 'done',
        completed_at: '2026-03-21T15:30:00Z',
      }
    },
  })
  .command('stats', {
    description: 'Show todo statistics',
    run() {
      return {
        total: 5,
        pending: 3,
        done: 2,
        by_priority: {
          high: 2,
          medium: 2,
          low: 1,
        },
      }
    },
  })
  .command('stream', {
    description: 'Stream progress updates (demo)',
    hint: 'Streams 5 progress events with 300ms delays.',
    async *run() {
      for (let i = 1; i <= 5; i++) {
        await new Promise((r) => setTimeout(r, 300))
        yield {
          event: 'progress',
          step: i,
          total: 5,
          message: `Processing batch ${i}...`,
        }
      }
      yield {
        event: 'complete',
        message: 'All batches processed successfully',
      }
    },
  })

await cli.serve()
