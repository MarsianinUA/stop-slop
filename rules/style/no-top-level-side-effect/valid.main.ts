// @path apps/api/src/main.ts
import { NestFactory } from '@nestjs/core';

import { AppModule } from './app.module';

export async function bootstrap(): Promise<void> {
  const app = await NestFactory.create(AppModule);
  await app.listen(3000);
}

void bootstrap();
