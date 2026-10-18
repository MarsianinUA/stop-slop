// @path src/__tests__/payments.spec.ts
import { Inject, Injectable } from '@nestjs/common';

@Injectable()
export class FakePayments {
  constructor(@Inject('PAYMENTS_CLIENT') private readonly client: unknown) {}

  touch(): unknown {
    return this.client;
  }
}
