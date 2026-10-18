import { forwardRef, Inject, Injectable } from '@nestjs/common';
import { InjectQueue } from '@nestjs/bullmq';
import { Inject as Use } from 'typedi';

import { Inject as LocalInject } from './inject';
import { PAYMENTS_CLIENT, PREFIX } from './payments.constants';

type PaymentsClient = { charge(total: number): Promise<void> };

@Injectable()
export class OrdersService {}

@Injectable()
export class PaymentsService {
  constructor(
    @Inject(PAYMENTS_CLIENT) private readonly client: PaymentsClient,
    @Inject(forwardRef(() => OrdersService)) private readonly orders: OrdersService,
    @Inject(`${PREFIX}_CLOCK`) private readonly clock: { now(): number },
    @Inject() private readonly plain: OrdersService,
    @InjectQueue('payments') private readonly queue: unknown,
    @Use('typedi-token') private readonly other: unknown,
    @LocalInject('local') private readonly local: unknown,
  ) {}

  touch(): unknown[] {
    return [this.client, this.orders, this.clock, this.plain, this.queue, this.other, this.local];
  }
}
