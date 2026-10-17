declare function load(id: string): Promise<string>;
declare const logger: { warn(message: string): void; error(message: string, cause: unknown): void };
declare const Logger: { warn(message: string): void };
declare const remoteLogger: { error(message: string): Promise<void> };

export async function warmUp(id: string): Promise<void> {
  try {
    await load(id);
  } catch {} // expect: no-empty-catch
}

export async function prefetch(id: string): Promise<void> {
  try {
    await load(id);
  } catch (error) { // expect: no-empty-catch
    // The cache fills itself on the next read.
  }
}

export async function refresh(id: string): Promise<void> {
  try {
    await load(id);
  } catch (error) { // expect: no-empty-catch
    logger.warn(`refresh failed for ${id}: ${String(error)}`);
  }
}

export async function report(id: string): Promise<void> {
  try {
    await load(id);
  } catch (error) { // expect: no-empty-catch
    console.error('report failed', error);
  }
}

export async function audit(id: string): Promise<void> {
  try {
    await load(id);
  } catch (error) { // expect: no-empty-catch
    Logger.warn(`audit failed for ${id}`);
    logger?.error('audit failed', error);
  }
}

export async function ship(id: string): Promise<void> {
  try {
    await load(id);
  } catch { // expect: no-empty-catch
    await remoteLogger.error(`ship failed for ${id}`);
  }
}

export class OrderSync {
  private readonly logger = logger;
  readonly #log = logger;

  async pull(id: string): Promise<void> {
    try {
      await load(id);
    } catch (error) { // expect: no-empty-catch
      this.logger.error(`pull failed for ${id}`, error);
    }
  }

  async push(id: string): Promise<void> {
    try {
      await load(id);
    } catch (error) { // expect: no-empty-catch
      this.#log.warn(`push failed for ${id}`);
    }
  }

  async retry(id: string): Promise<string> {
    try {
      return await load(id);
    } catch (error) {
      try {
        return await load(`${id}:retry`);
      } catch {} // expect: no-empty-catch
      throw error;
    }
  }
}
