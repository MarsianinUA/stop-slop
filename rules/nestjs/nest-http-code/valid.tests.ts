// @path src/__tests__/orders.e2e-spec.ts
import { Controller, Get } from '@nestjs/common';

@Controller('probe')
export class ProbeController {
  @Get()
  probe(): string {
    return 'ok';
  }
}
