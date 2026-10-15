function describe(value: object): string { // expect: no-object-parameters
  return String(value);
}

function consume(value: object = {}): void { // expect: no-object-parameters
  void value;
}

const read = (source: object | null): number => (source ? 1 : 0); // expect: no-object-parameters

class Registry {
  register(entry: (object), name: string): void { // expect: no-object-parameters
    void entry;
    void name;
  }
}

interface Sink {
  write(chunk: object): void; // expect: no-object-parameters
}

type Listener = (event: object) => void; // expect: no-object-parameters

type Bag = object;
function store(bag: Bag): void { // expect: no-object-parameters
  void bag;
}

const immediate = ((input: object) => input)({}); // expect: no-object-parameters

export type { Sink, Listener };
export { describe, consume, read, Registry, store, immediate };
