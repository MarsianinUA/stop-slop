declare function save(id: string): Promise<void>;
declare function load(id: string): Promise<string>;
declare function report(id: string, error: unknown): void;
declare function run(): Promise<void>;

export async function syncAll(orders: string[]): Promise<void> {
  const results = await Promise.allSettled(orders.map((order) => save(order)));
  results.forEach((result, index) => {
    if (result.status === 'rejected') {
      report(orders[index] ?? '', result.reason);
    }
  });
}

export async function pair(id: string): Promise<[string, void]> {
  return Promise.all([load(id), save(id)]);
}

export async function workers(): Promise<void> {
  await Promise.all(Array.from({ length: 4 }, run));
}

export async function stopAll(ids: string[]): Promise<void> {
  await Promise.all(ids.map((id) => save(id).catch((error: unknown) => report(id, error))));
}

export async function storeEach(ids: string[]): Promise<void> {
  await Promise.all(
    ids.map(async (id) => {
      try {
        await save(id);
      } catch (error) {
        report(id, error);
      }
    }),
  );
}

export async function first(ids: string[]): Promise<string> {
  return Promise.race(ids.map((id) => load(id)));
}


export async function keepEach(ids: string[]): Promise<void> {
  await Promise.all(
    ids.map(function (id) {
      return save(id).catch((error: unknown) => report(id, error));
    }),
  );
}

export async function waitEach(ids: string[]): Promise<void> {
  await Promise.all(
    ids.map(async (id) => {
      await save(id).catch((error: unknown) => report(id, error));
    }),
  );
}
