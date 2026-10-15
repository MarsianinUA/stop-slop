declare function load(): Promise<string>;
declare function log(message: string, cause: unknown): void;

export async function readOrder(): Promise<string | null> {
  try {
    return await load();
  } catch (error) {
    log('order', error);
    return null;
  }
}

export async function readCustomer(): Promise<string | null> {
  try {
    return await load();
  } catch (error: unknown) {
    log('customer', error);
    return null;
  }
}

export function parseLine(line: string): number {
  try {
    return JSON.parse(line);
  } catch (exception) {
    const error = new Error(`bad line: ${line}`);
    log(error.message, exception);
    return 0;
  }
}

export function report(error: string): void {
  try {
    log(error, null);
  } catch (reason) {
    log(error, reason);
  }
}

export async function retryLoad(): Promise<string> {
  try {
    return await load();
  } catch (first) {
    try {
      return await load();
    } catch (second) {
      log('retry', [first, second]);
      throw second;
    }
  }
}
