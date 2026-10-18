// @path src/orders/checkout.spec.ts
import { saveOrder } from './orders.repository';

interface Order {
  id: string;
}

export async function checkout(order: Order, note: string): Promise<string> {
  const receipt = `${note}:${Date.now()}`;
  await persist(order);
  return receipt;
}

async function persist(order: Order): Promise<void> {
  await saveOrder(order);
}
