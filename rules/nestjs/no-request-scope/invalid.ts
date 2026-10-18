import { Controller, Injectable, Module, Scope } from '@nestjs/common';
import type { Provider } from '@nestjs/common';
import { Scope as Lifetime } from '@nestjs/common';

@Injectable({ scope: Scope.REQUEST }) // expect: no-request-scope
export class RequestContext {
  readonly startedAt = Date.now();
}

@Injectable({ scope: Scope.REQUEST }) // expect: no-request-scope
export class AuditTrail {}

@Injectable({ scope: Lifetime.REQUEST, durable: true }) // expect: no-request-scope
export class TenantConnection {}

@Controller({ path: 'reports', scope: Scope['REQUEST'] }) // expect: no-request-scope
export class ReportsController {}

export const REPORTS = 'REPORTS';

const reportsProvider: Provider = { // expect: no-request-scope
  provide: REPORTS,
  useClass: TenantConnection,
  scope: Scope.REQUEST,
};

@Module({
  providers: [
    reportsProvider,
    { provide: 'AUDIT', useValue: [], scope: Scope.REQUEST }, // expect: no-request-scope
  ],
})
export class ReportsModule {}

export function scopeOf(isPerRequest: boolean): Scope {
  return isPerRequest ? Scope.REQUEST : Scope.DEFAULT; // expect: no-request-scope
}
