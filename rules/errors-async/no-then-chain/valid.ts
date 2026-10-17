declare function load(id: string): Promise<string>;
declare function report(error: unknown): void;
declare function release(): void;

export async function title(id: string): Promise<string> {
  const text = await load(id);
  return text.trim();
}

export function quiet(id: string): Promise<string | void> {
  return load(id).catch(report);
}

export function done(id: string): Promise<string> {
  return load(id).finally(release);
}

export const ready = {
  then(resolve: (value: number) => void): void {
    resolve(1);
  },
};
