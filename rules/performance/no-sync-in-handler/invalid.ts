import { readFileSync } from 'node:fs';
import * as fs from 'fs';
import * as zlib from 'zlib';
import { execSync } from 'child_process';
import { randomBytes } from 'crypto';
import bcrypt from 'bcrypt';
import { Controller, Get, Injectable } from '@nestjs/common';
import { MessagePattern } from '@nestjs/microservices';

@Controller('reports')
export class ReportsController {
  @Get(':id')
  find(): string {
    return readFileSync('report.txt', 'utf8'); // expect: no-sync-in-handler
  }

  @MessagePattern('reports.build')
  build(): string {
    return execSync('make report').toString(); // expect: no-sync-in-handler
  }
}

@Injectable()
export class ReportsService {
  load(name: string): string {
    return fs.readFileSync(name, 'utf8'); // expect: no-sync-in-handler
  }

  pack(data: Buffer): Buffer {
    return zlib.gzipSync(data); // expect: no-sync-in-handler
  }

  token(): string {
    return randomBytes(32).toString('hex'); // expect: no-sync-in-handler
  }

  hash(password: string): string {
    return bcrypt.hashSync(password, 10); // expect: no-sync-in-handler
  }

  exists(names: string[]): boolean {
    return names.every((name) => fs.existsSync(name)); // expect: no-sync-in-handler
  }
}
