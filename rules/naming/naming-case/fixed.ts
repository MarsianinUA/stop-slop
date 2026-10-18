import { readFileSync } from 'node:fs';

const PAGE_SIZE = 50;
export const retryLimit = 3;

type OrderRow = { id: string; total: number };
interface LineItem {
  sku: string;
}
enum OrderStatus {
  Open = 'open',
}

function formatTotal(row: OrderRow): string {
  return row.total.toFixed(2);
}

const to_label = (row: OrderRow): string => row.id;
export const labels = [to_label];

export function sum_lines(lines: LineItem[]): number {
  let runningTotal = 0;
  const maxLines = 100;
  for (const line of lines.slice(0, maxLines)) {
    runningTotal += line.sku.length;
  }
  return runningTotal;
}

export function read_status({ order_id: orderId }: { order_id: string }): OrderStatus {
  return orderId ? OrderStatus.Open : OrderStatus.Open;
}

export function first<Item>(items: Item[]): Item {
  return items[0];
}

export function describe_row(row: OrderRow): string {
  const row_id = row.id;
  const rowId = row.id.trim();
  return `${row_id}:${rowId}:${formatTotal(row)}`;
}

export function load(path: string): string {
  const fileText = readFileSync(path, 'utf8');
  return { file_text: fileText }.file_text.slice(0, PAGE_SIZE);
}

export class Cart {
  private item_count = 0;

  add(): number {
    this.item_count += 1;
    return this.item_count;
  }
}

const shared_label = 'cart';
export { shared_label };

const RUN_E2E_TIMEOUT_MS = 1000;
export const limits = [RUN_E2E_TIMEOUT_MS];

let RETRY_COUNT = 3;
export const retries = [RETRY_COUNT];

let pendingJob;
export const jobs = [pendingJob];
