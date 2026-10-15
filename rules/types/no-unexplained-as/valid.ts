import { readFile as read } from 'node:fs/promises';

type UserId = string & { readonly brand: 'UserId' };
interface User { id: UserId; name: string }
declare const input: unknown;
declare const raw: string;

// SAFETY: ids in the users table are checked on insert.
const id = raw as UserId;

const user = <User>input; // SAFETY: the schema above parsed `input`.

const fields = {
  id,
  name: 'Ann',
} as User; // SAFETY: the end of the last line counts.

const roles = ['admin', 'reader'] as const;
const point = <const>{ x: 1, y: 2 };
const checked = { id, name: 'Bob' } satisfies User;
const limits = { retries: 3 } as const satisfies Record<string, number>;

export { id as userId, user, fields, roles, point, checked, limits, read };
