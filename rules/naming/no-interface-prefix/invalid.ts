interface IOrder { // expect: no-interface-prefix
  id: string;
}

export interface ICustomer { // expect: no-interface-prefix
  name: string;
}

interface IShipment { // expect: no-interface-prefix
  id: string;
}

class Shipment implements IShipment {
  id = '';
}

interface IPayment { // expect: no-interface-prefix
  total: number;
}

interface IPayment {
  currency: string;
}

interface IRefund { // expect: no-interface-prefix
  amount: number;
}

export { type IRefund, Shipment };

export function total(order: IOrder, payment: IPayment): string {
  return `${order.id}:${payment.total}${payment.currency}`;
}
