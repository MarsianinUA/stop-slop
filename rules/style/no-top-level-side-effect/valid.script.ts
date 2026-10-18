// @path packages/db/scripts/migrate.ts
import { migrate } from '../src/migrate';

export async function main(): Promise<void> {
  await migrate();
}

void main().then(() => process.exit(process.exitCode ?? 0));
