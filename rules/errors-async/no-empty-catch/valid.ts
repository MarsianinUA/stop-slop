declare function load(id: string): Promise<string>;
declare function log(message: string): void;
declare const logger: { warn(message: string): void; flush(): void };
declare const metrics: { error(name: string): void };
declare const dialog: { error(message: string): void };

export async function read(id: string): Promise<string> {
  try {
    return await load(id);
  } catch (error) {
    logger.warn(`read failed for ${id}`);
    throw error;
  }
}

export async function readOrNull(id: string): Promise<string | null> {
  try {
    return await load(id);
  } catch {
    return null;
  }
}

export async function readOrEmpty(id: string): Promise<string> {
  try {
    return await load(id);
  } catch (error) {
    logger.warn(`read failed for ${id}: ${String(error)}`);
    return '';
  }
}

export async function count(id: string): Promise<void> {
  try {
    await load(id);
  } catch {
    metrics.error('load');
  }
}

export async function show(id: string): Promise<void> {
  try {
    await load(id);
  } catch {
    dialog.error(`Could not load ${id}`);
  }
}

export async function flush(id: string): Promise<void> {
  try {
    await load(id);
  } catch {
    logger.flush();
  }
}

export async function note(id: string): Promise<void> {
  try {
    await load(id);
  } catch (error) {
    log(`note failed for ${id}`);
  }
}

export async function describe(id: string): Promise<void> {
  try {
    await load(id);
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    logger.warn(message);
  }
}

export async function fallback(id: string): Promise<string> {
  let value = 'none';
  try {
    value = await load(id);
  } catch {
    value = 'default';
  }
  return value;
}

export function readLater(id: string): Promise<string | undefined> {
  return load(id).catch(() => undefined);
}

export function noteLater(id: string): void {
  void load(id).catch(log);
  void load(id).catch(logger.warn);
  void load(id).finally(() => {});
}

export function readOrThrow(id: string): Promise<string> {
  return load(id).catch((error: unknown) => {
    logger.warn(`read failed for ${id}`);
    throw error;
  });
}
