import { promises, readFileSync } from 'node:fs';
import { randomBytes } from 'crypto';
import { Controller, Get, Injectable, OnModuleInit } from '@nestjs/common';

const TEMPLATE = readFileSync('template.html', 'utf8');

@Injectable()
export class TemplatesService implements OnModuleInit {
  private readonly layout: string;
  private readonly footer = readFileSync('footer.html', 'utf8');
  private header = '';

  constructor() {
    this.layout = readFileSync('layout.html', 'utf8');
  }

  onModuleInit(): void {
    this.header = readFileSync('header.html', 'utf8');
  }

  async page(name: string): Promise<string> {
    const body = await promises.readFile(name, 'utf8');
    return this.header + this.layout + body + this.footer;
  }

  salt(done: (error: Error | null, buffer: Buffer) => void): void {
    randomBytes(16, done);
  }

  count(items: number[]): number {
    return countSync(items);
  }
}

@Controller('pages')
export class PagesController {
  @Get()
  home(): string {
    return TEMPLATE;
  }
}

export function loadConfig(): string {
  return readFileSync('config.json', 'utf8');
}

export class Plain {
  read(): string {
    return readFileSync('plain.txt', 'utf8');
  }
}

function countSync(items: number[]): number {
  return items.length;
}
