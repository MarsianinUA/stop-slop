// @path src/__tests__/orders.ts
interface Repository { find(id: string): unknown }

const repository = { find: () => null } as unknown as Repository;

export { repository };
