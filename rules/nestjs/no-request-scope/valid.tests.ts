// @path src/__tests__/request-context.spec.ts
import { Injectable, Scope } from '@nestjs/common';

@Injectable({ scope: Scope.REQUEST })
export class FakeRequestContext {}
