interface Order {
  status: string;
  items: string[];
  shippedAt: Date | null;
  isOnHold: boolean;
}
declare const order: Order;

export function ship(): string {
  const isPaid = order.status === 'paid';
  const hasItems = order.items.length > 0;
  const isUnshipped = order.shippedAt === null && !order.isOnHold;
  const isReadyToShip = isPaid && hasItems && isUnshipped;
  if (!isReadyToShip) {
    return 'wait';
  }
  if (isPaid && hasItems) {
    return 'ship';
  }
  if ((order.shippedAt ?? order.status ?? null) && isPaid) {
    return 'shipped';
  }
  return isPaid || isUnshipped ? 'ok' : 'no';
}

export function flags(): boolean {
  return order.isOnHold && order.items.length > 0 && order.status === 'paid';
}

export const open = [order].filter((one) => one.isOnHold && one.items.length > 0 && one.status === 'open');
