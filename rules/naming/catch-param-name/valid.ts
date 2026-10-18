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

export async function retryLoad(): Promise<string> {
  try {
    return await load();
  } catch (error) {
    try {
      return await load();
    } catch (retryError) {
      log('retry', [error, retryError]);
      throw retryError;
    }
  }
}

export function parse(line: string): number | null {
  try {
    return JSON.parse(line);
  } catch {
    return null;
  }
}

export function head(lines: string[]): string | null {
  try {
    return lines[0];
  } catch ({ message }) {
    log(String(message), null);
    return null;
  }
}

export function tryLog(line: string): boolean {
  try {
    log(line, null);
    return true;
  } catch (_error) {
    return false;
  }
}
