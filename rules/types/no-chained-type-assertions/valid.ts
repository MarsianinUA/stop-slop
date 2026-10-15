interface User { id: string }
declare const input: string;
declare const logger: { child(): unknown };

const single = input as unknown;
const wrapped = (input as unknown);
const frozen = ({ id: 'a' } as const) as const;
const checked = (input as unknown) satisfies unknown;

// SAFETY: pino's child logger has the custom level, which its types do not declare.
const notice = logger.child() as unknown as { notice(message: string): void };

export { single, wrapped, frozen, checked, notice };
