// @path src/__tests__/discovery.spec.ts
import { ConfigService } from '@nestjs/config';

export function baseUrl(config: ConfigService): string | undefined {
  return config.get('app.baseUrl');
}
