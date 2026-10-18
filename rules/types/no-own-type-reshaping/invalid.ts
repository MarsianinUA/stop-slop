// @alias @/*
// @workspace @acme/db
import type { Order } from './order.types';
import type { Customer } from '@/customers/customer.types';
import type { InvoiceRow } from '@acme/db';
import type * as models from './models';

interface Line {
  sku: string;
  quantity: number;
}

enum Status {
  Open = 'open',
  Closed = 'closed',
  Archived = 'archived',
}

export type OrderDraft = Omit<Order, 'id'>; // expect: no-own-type-reshaping
export type CustomerCard = Pick<Customer, 'name' | 'email'>; // expect: no-own-type-reshaping

export function patchLine(line: Line, patch: Partial<Line>): Line { // expect: no-own-type-reshaping
  return { ...line, ...patch };
}

export function finalize(row: Required<InvoiceRow>): Readonly<models.Receipt> { // expect: no-own-type-reshaping x2
  return row.receipt;
}

export function reopen(status: Exclude<Status, Status.Archived>): Extract<Status, Status.Open> { // expect: no-own-type-reshaping x2
  return status === Status.Open ? status : Status.Open;
}

export function withTotal(order: Order & { total: number }): NonNullable<Order['customer']> { // expect: no-own-type-reshaping x2
  return order.customer ?? order.fallbackCustomer;
}

export type Event =
  | ({ kind: 'line' } & Line) // expect: no-own-type-reshaping
  | { kind: 'empty' };

export type OrderUpsert = Omit<Order, 'note'> & { note: string | null }; // expect: no-own-type-reshaping

export interface OrderPreview extends Pick<Order, 'id'> { // expect: no-own-type-reshaping
  label: string;
}

export class LineDraft implements Partial<Line> { // expect: no-own-type-reshaping
  sku = '';
}
