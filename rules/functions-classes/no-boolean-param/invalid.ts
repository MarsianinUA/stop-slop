declare function send(text: string): void;

export function notify(text: string, isUrgent: boolean): void { // expect: no-boolean-param
  if (isUrgent) {
    send(`URGENT ${text}`);
    return;
  }
  send(text);
}

export function label(count: number, plural = false): string { // expect: no-boolean-param
  return plural ? `${count} items` : `${count} item`;
}

export function track(event: string, debug: boolean): void { // expect: no-boolean-param
  debug && send(event);
}

export class Exporter {
  private readonly write = (line: string, verbose?: true | false): void => { // expect: no-boolean-param
    if (!verbose) {
      return;
    }
    send(line);
  };

  rows(names: string[], withHeader: boolean | undefined): string[] { // expect: no-boolean-param
    this.write(names.join(','), withHeader);
    return withHeader && names.length > 0 ? ['name', ...names] : names;
  }
}

export const drain = function (queue: string[], once: boolean): number { // expect: no-boolean-param
  while (!once && queue.length > 0) {
    queue.pop();
  }
  return queue.length;
};
