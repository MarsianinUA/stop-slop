// @path apps/api/src/scripts-legacy/register-domain-handlers.ts
import { registerFatalErrorLogging } from '@workspace/shared/logging';

export const HANDLERS = ['orders', 'payments'];

registerFatalErrorLogging(HANDLERS); // expect: no-top-level-side-effect
