// @path src/health/health.controller.ts
import { Controller, Get } from '@nestjs/common';

enum HttpStatus {
  OK = 200,
}

@Controller('health')
export class HealthController {
  @Get() // expect: nest-http-code
  check(): { status: HttpStatus } {
    return { status: HttpStatus.OK };
  }
}
