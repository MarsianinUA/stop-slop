export enum Status {
  Active = 'active',
  Closed = 'closed',
}

export enum ApiRouteId {
  MarketsGetById = 'markets.get_by_id',
}

export enum ApiKeyErrorCode {
  ApiKeyRevoked = 1,
  NotConfigured = 2,
}

export enum Direction {
  Up,
  Down,
}

export enum Unit {
  Unit = 'unit',
}

export enum HttpCode {
  Code_404 = 404,
}

export enum Order {
  Ascending = 'asc',
  'order-desc' = 'desc',
}

declare enum Legacy {
  LegacyValue,
}

export const status = { StatusActive: 'active' } as const;
export const legacy = Legacy.LegacyValue;
