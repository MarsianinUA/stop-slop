declare function load(id: string): Promise<string>;
declare function release(): void;
declare const client: { send(body: string): Promise<void> } | undefined;

export async function read(id: string): Promise<string> {
  try {
    return load(id); // expect: return-await-in-try
  } catch {
    return '';
  }
}

export async function send(body: string): Promise<void> {
  try {
    if (body.length > 0) {
      return client?.send(body); // expect: return-await-in-try
    }
  } finally {
    release();
  }
}

export const readTyped = async (id: string): Promise<string> => {
  try {
    return load(`typed:${id}`) as Promise<string>; // expect: return-await-in-try
  } catch {
    return 'typed';
  }
};

async function cached(id: string): Promise<string> {
  return `cached:${id}`;
}

export class Reader {
  async read(id: string): Promise<string> {
    try {
      return cached(id); // expect: return-await-in-try
    } catch {
      return 'none';
    }
  }
}
