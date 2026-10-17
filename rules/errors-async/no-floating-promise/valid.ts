import { publish } from './events';

async function save(id: string): Promise<void> {
  await Promise.resolve(id);
}

function count(id: string): number {
  return id.length;
}

async function* stream(ids: string[]): AsyncGenerator<string> {
  for (const id of ids) {
    yield id;
  }
}

let handler = async (id: string): Promise<void> => {
  await save(id);
};
handler = async () => {};

export async function run(id: string): Promise<void> {
  await save(id);
  void save(`void:${id}`);
  const pending = save(`later:${id}`);
  save(`caught:${id}`).catch(() => undefined);
  count(id);
  publish(id);
  stream([id]);
  handler(id);
  [id].forEach((each) => save(each));
  await pending;
}

export function start(id: string): Promise<void> {
  return save(id);
}
