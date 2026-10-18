import { Controller, Get, Post } from '@nestjs/common';
import { MessagePattern } from '@nestjs/microservices';
import { Get as Route } from 'routing-controllers';

const ROUTE = 'legacyRoute';

@Controller('v1/api-keys')
export class ApiKeysController {
  @Get()
  list(): string[] {
    return [];
  }

  @Get('/')
  root(): string {
    return '';
  }

  @Get(':keyId/usage-stats')
  usage(): string {
    return '';
  }

  @Get(':chainId/:marketAddress/history')
  history(): string {
    return '';
  }

  @Get(['llms.txt', 'llms-all.txt', '.well-known/security.txt', 'v2'])
  files(): string {
    return '';
  }

  @Get('files/*filePath')
  file(): string {
    return '';
  }

  @Get('archive/./..')
  parent(): string {
    return '';
  }

  @Get('users{/:userId}')
  user(): string {
    return '';
  }

  @Get('ab+cd/(.*)')
  pattern(): string {
    return '';
  }

  @Post(ROUTE)
  legacy(): void {}

  @Post(`${ROUTE}/runNow`)
  run(): void {}

  @MessagePattern('orders.getAll')
  message(): void {}

  @Route('/camelCase')
  other(): void {}
}

@Controller({ host: ':account.example.com' })
export class HostController {}
