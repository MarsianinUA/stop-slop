import { HttpException } from './http-exception';

declare function retryable<T extends new (...args: never[]) => Error>(base: T): T;

export class NotFoundError extends Error {}

export class ConflictError extends HttpException {}

class AppError extends Error {}

export class OrderError extends AppError {}

export class RetryError extends retryable(Error) {}

export const ValidationError = class extends Error {};

export class ErrorBoundary {}

export class OrderErrorMapper {}

export interface ApiError {
  code: number;
}

declare class LegacyError {}

declare namespace Legacy {
  class ClientError {}
}

export { LegacyError, Legacy };
