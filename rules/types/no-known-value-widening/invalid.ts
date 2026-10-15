type Command = () => void;
const start: Command = () => {};

const value: unknown = { id: 1 }; // expect: no-known-value-widening
const shape: object = [1, 2]; // expect: no-known-value-widening
const commands: Record<string, Command> = { start }; // expect: no-known-value-widening
const indexed: { [name: string]: Command } = { begin: start }; // expect: no-known-value-widening
const mapped: { [K in string]: Command } = { run: start }; // expect: no-known-value-widening
const frozen: Readonly<Record<string, number>> = { a: 1 }; // expect: no-known-value-widening
const label: unknown = 'ready'; // expect: no-known-value-widening
const table: Record<string, number> = { // expect: no-known-value-widening
  mainnet: 1,
  base: 8453,
};

const source = { start };
const fromBinding: Record<string, Command> = source; // expect: no-known-value-widening

const asserted = { launch: start } as Record<string, Command>; // expect: no-known-value-widening
const opaque = 'text' as unknown; // expect: no-known-value-widening
const castTwice: unknown = { cast: 1 } as unknown; // expect: no-known-value-widening

class Registry {
  commands: Record<string, Command> = { boot: start }; // expect: no-known-value-widening
}

let later: Record<string, Command>;
later = { open: start }; // expect: no-known-value-widening

let status: unknown = 'idle'; // expect: no-known-value-widening
status = 'busy'; // expect: no-known-value-widening

// `Record<string, unknown>` is the type of the whole pattern, so `part = { id: 1 }` widens nothing.
let { part }: Record<string, unknown> = { part: 1 }; // expect: no-known-value-widening
part = { id: 1 };

function create(): Record<string, Command> {
  return { start }; // expect: no-known-value-widening
}

function pick(flag: boolean): unknown {
  if (flag) {
    return { id: 1 }; // expect: no-known-value-widening
  }
  return [1]; // expect: no-known-value-widening
}

const arrow = (): object => ({ id: 2 }); // expect: no-known-value-widening

export { value, shape, commands, indexed, mapped, frozen, label, table, fromBinding };
export { asserted, opaque, castTwice, Registry, later, status, part, create, pick, arrow };
