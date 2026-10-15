import { readFile } from 'fs';
import { Channel } from './channel.enum';

enum Source {
  Dashboard = 'DASHBOARD',
  Api = 'API',
}

enum Builtin {
  Fs = 'fs',
}

enum Count {
  One = 1,
}

interface Event {
  source: 'DASHBOARD' | 'API';
}

declare function sync(source: Source, channel: Channel): void;
declare function sql(parts: TemplateStringsArray): string;
declare const table: Record<string, number>;

sync(Source.Dashboard, Channel.Email);
const keys = { DASHBOARD: 1, 'API': 2 };
const fromTable = table['DASHBOARD'];
const query = sql`DASHBOARD`;
const counted = '1';
const greeting = `hello ${Source.Api}`;
const lower = 'dashboard';
const email = 'email';
const inTable = 'DASHBOARD' in table;
let total = 0;
({ 'API': total } = table);

export { readFile, Builtin, Count, keys, fromTable, query, counted, greeting, lower, email, inTable, total };
export type { Event };
