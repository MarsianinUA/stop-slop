declare function load(id: string): Promise<string>;

function toView(text: string): string {
  return text.trim();
}

export async function read(id: string): Promise<string> {
  try {
    return await load(id);
  } catch {
    return '';
  }
}

export async function view(id: string): Promise<string> {
  try {
    return toView(await load(id));
  } catch {
    return '';
  }
}

export async function after(id: string): Promise<string> {
  try {
    await load(`warm:${id}`);
  } catch {
    return '';
  }
  return load(id);
}

export async function fallback(id: string): Promise<string> {
  try {
    return await load(id);
  } catch {
    return load(`backup:${id}`);
  }
}

export function plain(id: string): Promise<string> {
  try {
    return load(id);
  } catch {
    return Promise.resolve('');
  }
}

export async function each(ids: string[]): Promise<string[]> {
  try {
    return await Promise.all(ids.map(async (id) => {
      return load(id);
    }));
  } catch {
    return [];
  }
}

export async function value(text: string): Promise<string> {
  try {
    return text;
  } finally {
    await load('log');
  }
}
