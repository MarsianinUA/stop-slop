// @alias @/*
import { archiveOrder } from '@/orders/archive';
import { saveOrder } from './orders.repository';

interface Order {
  id: string;
  total: number;
}

interface Ledger {
  write(order: Order, note: string): void;
}

export async function checkout(order: Order, note: string): Promise<string> { // expect: no-pass-through-param
  const receipt = `${note}:${Date.now()}`;
  await persist(order);
  return receipt;
}

async function persist(order: Order): Promise<void> {
  await saveOrder(order);
}

export class Billing {
  constructor(private readonly ledger: Ledger) {}

  charge(order: Order, note: string, attempt: number): number { // expect: no-pass-through-param x2
    const delayMs = attempt * 100;
    this.record(order, note);
    return delayMs;
  }

  private record(order: Order, note: string): void {
    this.ledger.write(order, note);
  }
}

export const retry = async (order: Order, times: number): Promise<number> => { // expect: no-pass-through-param
  for (let attempt = 0; attempt < times; attempt += 1) {
    await persist(order);
  }
  return times;
};

async function archive(order: Order): Promise<void> {
  await archiveOrder(order);
}

export async function close(order: Order, reason: string): Promise<string> { // expect: no-pass-through-param
  await archive(order);
  return reason;
}
