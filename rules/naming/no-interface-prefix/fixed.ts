interface Order {
  id: string;
}

export interface ICustomer {
  name: string;
}

interface IShipment {
  id: string;
}

class Shipment implements IShipment {
  id = '';
}

interface Payment {
  total: number;
}

interface Payment {
  currency: string;
}

interface IRefund {
  amount: number;
}

export { type IRefund, Shipment };

export function total(order: Order, payment: Payment): string {
  return `${order.id}:${payment.total}${payment.currency}`;
}
