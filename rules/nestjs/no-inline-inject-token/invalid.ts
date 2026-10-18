import { Inject, Injectable, Optional } from '@nestjs/common';
import { Inject as Use } from '@nestjs/common';

type PaymentsClient = { charge(total: number): Promise<void> };
type Clock = { now(): number };

@Injectable()
export class PaymentsService {
  @Inject('AUDIT_LOG') // expect: no-inline-inject-token
  private readonly audit!: { write(line: string): void };

  constructor(
    @Inject('PAYMENTS_CLIENT') private readonly client: PaymentsClient, // expect: no-inline-inject-token
    @Optional() @Inject(`CLOCK`) private readonly clock: Clock | null, // expect: no-inline-inject-token
    @Use('RETRY_POLICY') // expect: no-inline-inject-token
    private readonly retries: number,
  ) {}

  async pay(total: number): Promise<void> {
    this.audit.write(`pay ${total} at ${this.clock?.now() ?? 0} with ${this.retries}`);
    await this.client.charge(total);
  }
}
