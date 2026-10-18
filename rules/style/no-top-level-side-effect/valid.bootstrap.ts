// @path apps/api/src/bootstrap-env.ts
import { loadMonorepoDotEnv } from '@workspace/shared/env';

export const ENV_FILES = ['.env', '.env.local'];

loadMonorepoDotEnv(ENV_FILES);
