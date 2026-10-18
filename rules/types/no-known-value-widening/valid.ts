type Command = () => void;
interface Commands { readonly start: Command }
declare function make(): Record<string, Command>;
declare const input: unknown;
const start: Command = () => {};

const commands = { start };
const frozen = { start } as const;
const checked = { start } satisfies Record<string, Command>;
const empty: Record<string, Command> = {};
const accumulated = {} as Record<string, Command>;
const fromCall: Record<string, Command> = make();
const fromInput: unknown = input;
const labels: Record<'a' | 'b', number> = { a: 1, b: 2 };
const nothing: unknown = null;
const named: Commands = { start };
const fromNamed: Record<string, Command> = named;

let failure: unknown = null;
failure = input;

let lastError: unknown = 'none';
lastError = input;

declare const env: Record<string, string>;
declare const key: string;
const merged: Record<string, string> = { ...env };
const keyed: Record<string, number> = { [key]: 1 };

class Loader {
  private state: unknown = 'idle';

  load(): void {
    this.state = input;
  }
}

class Table {
  private readonly rows: Record<string, number> = { a: 1 };

  constructor(seed: Record<string, number>) {
    this.rows = seed;
  }
}

class Counter {
  #count: unknown = 1;

  bump(): void {
    this.#count = Number(this.#count) + 1;
  }
}

// Another file may write `token`.
export class Session {
  token: unknown = 'none';
}

function normalize(value: unknown): unknown {
  if (value === null) {
    return 'null';
  }
  return value;
}

function create(): Commands {
  return { start };
}

const chained = 'text' as unknown as number;

// SAFETY: plugins add their commands to this registry at startup.
const seeded = { start } as Record<string, Command>;

// SAFETY: the plugin API takes any value here.
const hidden = { id: 1 } as unknown;
const passed: unknown = hidden;

export { commands, frozen, checked, empty, accumulated, fromCall, fromInput, labels, nothing };
export { fromNamed, failure, lastError, merged, keyed, normalize, create, chained, seeded };
export { Loader, Table, Counter, passed };
