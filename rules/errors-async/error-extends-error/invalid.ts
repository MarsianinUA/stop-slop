export class OrderError { // expect: error-extends-error
  constructor(readonly orderId: string) {}
}

class BaseError {} // expect: error-extends-error

export class PaymentError extends BaseError {} // expect: error-extends-error

class Failure {}

export class ParseError extends Failure {} // expect: error-extends-error

export const TimeoutError = class { // expect: error-extends-error
  readonly retryable = true;
};

export abstract class DomainError { // expect: error-extends-error
  abstract readonly code: number;
}

export default class HTTPError {} // expect: error-extends-error

export const NotFoundError = class NotFound {}; // expect: error-extends-error
