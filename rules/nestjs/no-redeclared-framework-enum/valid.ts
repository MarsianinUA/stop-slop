import { HttpStatus } from '@nestjs/common';
import * as common from '@nestjs/common';

export enum OrderStatus {
  Open = 'open',
  Closed = 'closed',
}

export type HttpStatusCode = number;

export const Statuses = common.HttpStatus;

const { HttpStatus: Status } = common;

export function isMissing(status: HttpStatus): boolean {
  return status === HttpStatus.NOT_FOUND || status === Status.GONE;
}

declare module '@nestjs/common' {
  enum HttpStatus {
    TOO_EARLY = 425,
  }
}
