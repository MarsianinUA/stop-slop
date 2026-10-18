declare function load(): Promise<string>;
declare function log(message: string, cause: unknown): void;

export async function readOrder(): Promise<string | null> {
  try {
    return await load();
  } catch (err) { // expect: catch-param-name
    log('order', err);
    return null;
  }
}

export async function readCustomer(): Promise<string | null> {
  try {
    return await load();
  } catch (e: unknown) { // expect: catch-param-name
    log('customer', e);
    return null;
  }
}

export function parseLine(line: string): number {
  try {
    return JSON.parse(line);
  } catch (exception) { // expect: catch-param-name
    const error = new Error(`bad line: ${line}`);
    log(error.message, exception);
    return 0;
  }
}

export function report(error: string): void {
  try {
    log(error, null);
  } catch (reason) { // expect: catch-param-name
    log(error, reason);
  }
}

export async function retryLoad(): Promise<string> {
  try {
    return await load();
  } catch (first) { // expect: catch-param-name
    try {
      return await load();
    } catch (second) { // expect: catch-param-name
      log('retry', [first, second]);
      throw second;
    }
  }
}
