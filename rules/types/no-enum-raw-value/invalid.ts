enum Source {
  Dashboard = 'DASHBOARD',
  Api = "API",
  Import = `IMPORT`,
}

export enum Channel {
  Email = 'email',
}

declare function sync(source: string, channel: string): void;
declare const row: { source: string; channel: string };

sync('DASHBOARD', 'email'); // expect: no-enum-raw-value x2
export const payload = { source: "API", channel: Channel.Email }; // expect: no-enum-raw-value
if (row.source === `IMPORT`) { // expect: no-enum-raw-value
  sync(Source.Api, Channel.Email);
}

export function label(source: string): string {
  switch (source) {
    case 'DASHBOARD': // expect: no-enum-raw-value
      return 'Dashboard';
    default:
      return source;
  }
}

export function defaults(channel = 'email'): string[] { // expect: no-enum-raw-value
  return ['DASHBOARD', 'API', channel]; // expect: no-enum-raw-value x2
}

namespace Kind {
  export const all = 2;
}

enum Kind {
  Bulk = 'bulk',
}

export const kinds = [Kind.all, 'bulk']; // expect: no-enum-raw-value
