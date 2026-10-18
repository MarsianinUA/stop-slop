async function save(id: string): Promise<void> {
  await Promise.resolve(id);
}

function loadLater(id: string): Promise<string> {
  return Promise.resolve(id);
}

const notify = async (id: string): Promise<void> => {
  await save(`notify:${id}`);
};

export function run(id: string): void {
  save(id); // expect: no-floating-promise
  loadLater(id); // expect: no-floating-promise
  notify(id); // expect: no-floating-promise
  (async () => { // expect: no-floating-promise
    await save(`later:${id}`);
  })();
  (async function () { // expect: no-floating-promise
    await save(`soon:${id}`);
  })();
}

export class Ticker {
  async tick(): Promise<void> {
    await save('tick');
  }

  async flush(): Promise<void> {
    await save('flush');
  }

  start(): void {
    this.flush(); // expect: no-floating-promise
    setInterval(() => {
      this.tick(); // expect: no-floating-promise
    }, 1000);
  }
}

function store(id: string): Promise<void>;
function store(id: string, force: boolean): Promise<void>;
async function store(id: string, force = false): Promise<void> {
  await save(`${id}:${String(force)}`);
}

export function maybe(id: string, ready: boolean, cached?: string): void {
  ready && save(`and:${id}`); // expect: no-floating-promise
  ready || notify(`or:${id}`); // expect: no-floating-promise
  cached ?? save(`fallback:${id}`); // expect: no-floating-promise
  ready ? save(`yes:${id}`) : loadLater(`no:${id}`); // expect: no-floating-promise x2
  (save(`first:${id}`), notify(`second:${id}`)); // expect: no-floating-promise x2
  store(id); // expect: no-floating-promise
}

export class Archive {
  async #persist(): Promise<void> {
    await save('persist');
  }

  close(): void {
    this.#persist(); // expect: no-floating-promise
  }
}

export function soon(id: string): void {
  notify?.(`soon:${id}`); // expect: no-floating-promise
}
