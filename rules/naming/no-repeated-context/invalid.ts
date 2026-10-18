export enum Status {
  StatusActive = 'active', // expect: no-repeated-context
  Closed = 'closed',
}

export enum OrderStatus {
  OrderStatusPaid = 'paid', // expect: no-repeated-context
  StatusRefunded = 'refunded', // expect: no-repeated-context
  ShippedOrderStatus = 'shipped', // expect: no-repeated-context
  Open = 'open',
}

export enum Roles {
  RoleAdmin, // expect: no-repeated-context
  Viewer,
}

export const enum Theme {
  DarkTheme = 'dark', // expect: no-repeated-context
  Light = 'light',
}
