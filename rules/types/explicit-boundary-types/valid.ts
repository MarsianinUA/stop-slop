interface User { id: string }
type Handler = (event: string) => number;
declare const users: User[];
declare function wrap<T>(value: T): T;

export function findUser(id: string): User | undefined {
  return users.find((user) => user.id === id);
}

export const countUsers = (filter: string): number =>
  users.filter((user) => user.id === filter).length;

export const handler: Handler = (event) => event.length;

export const wrapped = wrap((value: string) => value);

export function page(limit = 10, label = 'all', since = new Date(), offset = -1): string {
  return `${label}${limit}${since.getTime()}${offset}`;
}

export function parse(input: string): number;
export function parse(input: number): number;
export function parse(input: string | number) {
  return Number(input);
}

function localHelper(value) {
  return value;
}

const localArrow = (value) => value;

class LocalService {
  run(value) {
    return value;
  }
}

export const helpers = [localHelper, localArrow, LocalService];

export const api = {
  get(id) {
    return id;
  },
};

export class UserService {
  #cache = new Map<string, User>();

  constructor(private readonly repository: User[]) {}

  find(id: string): User | undefined {
    return this.lookup(id) ?? this.#read(id);
  }

  get size(): number {
    return this.repository.length;
  }

  set label(value: string) {
    void value;
  }

  handle: Handler = (event) => event.length;

  private lookup(id) {
    return this.#cache.get(id);
  }

  protected reload() {
    return this.repository;
  }

  #read(id) {
    return this.repository.find((user) => user.id === id);
  }
}
