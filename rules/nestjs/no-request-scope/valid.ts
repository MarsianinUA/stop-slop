import { Injectable, Scope } from '@nestjs/common';
import { Scope as SentryScope } from '@sentry/node';

enum Access {
  REQUEST = 'request',
  GRANT = 'grant',
}

@Injectable()
export class ReportsService {}

@Injectable({ scope: Scope.DEFAULT })
export class DefaultService {}

@Injectable({ scope: Scope.TRANSIENT })
export class TransientService {}

export function tag(scope: SentryScope): void {
  scope.setTag('kind', 'report');
}

export function label(access: Access, local: { REQUEST: string }): string {
  return access === Access.REQUEST ? local.REQUEST : 'none';
}

export const SENTRY_REQUEST = (SentryScope as unknown as { REQUEST: string }).REQUEST;
