declare function save(id: string): Promise<void>;
declare function notify(userId: string): Promise<void>;
declare function isActive(userId: string): boolean;

export class OrderSync {
  constructor(private readonly ids: string[]) {}

  async syncAll(orders: string[]): Promise<void> {
    await Promise.all(orders.map((order) => save(order))); // expect: bulk-all-settled
  }

  async notifyAll(): Promise<void> {
    await Promise.all(this.ids.map(notify)); // expect: bulk-all-settled
  }
}

export function storeAll(ids: string[]): Promise<void[]> {
  return Promise.all( // expect: bulk-all-settled
    ids.map(async (id) => {
      await save(id);
    }),
  );
}

export async function remind(userIds: string[]): Promise<void> {
  await Promise.all( // expect: bulk-all-settled
    userIds
      .filter(isActive)
      .map((userId) => notify(userId)),
  );
}
