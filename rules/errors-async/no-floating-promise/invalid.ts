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
