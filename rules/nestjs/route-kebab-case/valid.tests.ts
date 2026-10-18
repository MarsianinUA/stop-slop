// @path src/__tests__/routes.e2e-spec.ts
import { Controller, Get } from '@nestjs/common';

@Controller('probeRoutes')
export class ProbeController {
  @Get('slowPath')
  slow(): string {
    return '';
  }
}
