import {
  Controller,
  Get,
  HttpCode,
  HttpCode as Status,
  HttpStatus,
  Post,
  Redirect,
  Sse,
} from '@nestjs/common';
import { MessagePattern } from '@nestjs/microservices';
import { Get as Route } from 'routing-controllers';
import { Observable, of } from 'rxjs';

function Fetch(): MethodDecorator {
  return () => undefined;
}

@Controller('reports')
export class ReportsController {
  @Get()
  @HttpCode(HttpStatus.OK)
  list(): string[] {
    return [];
  }

  @HttpCode(HttpStatus.ACCEPTED)
  @Post('rebuild')
  rebuild(): { jobId: string } {
    return { jobId: 'job-1' };
  }

  @Post('archive')
  @Status(204)
  archive(): void {}

  @Get('docs')
  @Redirect('https://docs.example.com', 301)
  docs(): void {}

  @Sse('events')
  events(): Observable<MessageEvent> {
    return of();
  }

  @MessagePattern('reports.count')
  count(): number {
    return 0;
  }

  @Route('/legacy')
  legacy(): string {
    return 'legacy';
  }

  @Fetch()
  fetch(): string {
    return 'fetched';
  }

  @Get('static')
  static ping(): string {
    return 'pong';
  }

  summary(): string {
    return 'summary';
  }
}
