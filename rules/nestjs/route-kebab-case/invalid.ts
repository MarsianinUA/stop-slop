import { Controller, Delete, Get, Patch, Post, Sse } from '@nestjs/common';
import { Controller as Routes } from '@nestjs/common';
import { Observable, of } from 'rxjs';

@Controller('v1/apiKeys') // expect: route-kebab-case
export class ApiKeysController {
  @Get(':keyId/usageStats') // expect: route-kebab-case
  usage(): string {
    return 'usage';
  }

  @Post('rotate_secret') // expect: route-kebab-case
  rotate(): void {}

  @Patch(['policy', 'Policy']) // expect: route-kebab-case
  policy(): void {}

  @Delete(`cache/allEntries`) // expect: route-kebab-case
  clear(): void {}

  @Sse('liveEvents') // expect: route-kebab-case
  events(): Observable<string> {
    return of();
  }

  @Get('LLMS.txt') // expect: route-kebab-case
  llms(): string {
    return '';
  }

  @Get('reports/dailySummary/byMarket') // expect: route-kebab-case x2
  summary(): string {
    return '';
  }
}

@Routes({ path: 'adminTools', version: '1' }) // expect: route-kebab-case
export class AdminToolsController {}
