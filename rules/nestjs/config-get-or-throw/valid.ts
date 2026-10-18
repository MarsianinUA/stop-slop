import { Injectable } from '@nestjs/common';
import type { INestApplication } from '@nestjs/common';
import { ConfigService } from '@nestjs/config';
import { Cache } from 'cache-manager';

import { ConfigService as LocalConfig } from './local-config.service';

@Injectable()
export class DiscoveryService {
  private readonly peppers = new Map<number, string>();
  private config: LocalConfig;

  constructor(
    private readonly settings: ConfigService,
    private readonly cache: Cache,
    local: LocalConfig,
  ) {
    this.config = local;
  }

  realtimePath(): string {
    return this.settings.getOrThrow<string>('realtime.path');
  }

  baseUrl(): string | null {
    return this.settings.getOrThrow<string | null>('app.baseUrl');
  }

  pepper(version: number): string | undefined {
    return this.peppers.get(version);
  }

  async cached(key: string): Promise<unknown> {
    return this.cache.get(key);
  }

  local(): string | undefined {
    return this.config.get('app.local');
  }

  static fromOther(other: { get(key: string): string }): string {
    return other.get('key');
  }
}

export function port(app: INestApplication, untyped: { get(key: string): number }): number {
  const configService = app.get(ConfigService);
  const service = app.get(DiscoveryService);
  void service;
  return configService.getOrThrow<number>('worker.port') + untyped.get('offset');
}

export function destructured(app: INestApplication): unknown {
  const { cache } = app.get(ConfigService) as unknown as { cache: Map<string, unknown> };
  return cache.get('key');
}

export class Settings {
  private readonly options: { get(key: string): string };

  constructor(options: ConfigService) {
    this.options = { get: (key) => options.getOrThrow<string>(key) };
  }

  read(key: string): string {
    return this.options.get(key);
  }
}

export function reassigned(app: INestApplication, other: { get(key: string): unknown }): unknown {
  let source = app.get(ConfigService) as { get(key: string): unknown };
  source = other;
  return source.get('key');
}

export function shadow(settings: Map<string, string>): string | undefined {
  return settings.get('ConfigService');
}
