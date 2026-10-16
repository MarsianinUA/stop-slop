declare function send(text: string): void;
declare function legacy(flag: boolean): void;
declare function Query(name: string): ParameterDecorator;

export interface Logger {
  log(line: string, verbose: boolean): void;
}

export type Filter = (value: string, strict: boolean) => boolean;

export function remember(flags: Map<string, boolean>, enabled: boolean): void {
  flags.set('enabled', enabled);
}

export function toText(value: boolean): string {
  return String(value);
}

export function either(value: boolean | string): string {
  return value ? 'yes' : 'no';
}

export function run({ dryRun = false }: { dryRun?: boolean }): string {
  return dryRun ? 'dry' : 'real';
}

export const counts = [true, false].map((done: boolean) => (done ? 1 : 0));

export const handlers = {
  toggle(enabled: boolean): string {
    return enabled ? 'on' : 'off';
  },
};

export class Toggle {
  private state = false;

  constructor(private readonly strict: boolean) {
    if (strict) {
      this.state = true;
    }
  }

  set active(value: boolean) {
    if (value || this.strict) {
      this.state = true;
    }
  }

  list(@Query('all') all: boolean): string {
    legacy(this.state);
    return all ? 'all' : 'some';
  }
}

export class ConsoleLogger implements Logger {
  log(line: string, verbose: boolean): void {
    if (verbose) {
      send(line);
    }
  }
}

export abstract class Job {
  abstract run(force: boolean): void;
}

export class SyncJob extends Job {
  override run(force: boolean): void {
    if (force) {
      send('sync');
    }
  }
}

export const filter: Filter = (value: string, strict: boolean) => (strict ? value.length > 0 : true);

export type Render = (value: string, loud: boolean) => string;

export function renderer(prefix: string): Render {
  return (value: string, loud: boolean) => (loud ? `${prefix}${value}!` : value);
}

export const shout = (): Render => (value: string, loud: boolean) =>
  loud ? value.toUpperCase() : value;

export const quiet = ((value: string, loud: boolean) => (loud ? value : value.toLowerCase())) as Render;

export class FieldLogger implements Logger {
  log = (line: string, verbose: boolean): void => {
    if (verbose) {
      send(line);
    }
  };
}
