import { OrderNotFound } from './errors/order.errors';

declare function find(id: string): string | undefined;
declare function save(id: string): Promise<void>;

export function order(id: string): string {
  const found = find(id);
  if (found === undefined) {
    throw OrderNotFound(id);
  }
  return found;
}

export async function store(id: string): Promise<void> {
  try {
    await save(id);
  } catch (error) {
    throw error;
  }
}

export function missing(id: string): Error {
  return new Error(`order ${id} is missing`);
}

export function rejected(id: string): Promise<never> {
  return Promise.reject(new Error(`order ${id} was rejected`));
}
