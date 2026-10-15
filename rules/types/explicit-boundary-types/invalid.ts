interface User { id: string }
declare const users: User[];

export function findUser(id: string) { // expect: explicit-boundary-types
  return users.find((user) => user.id === id);
}

export async function loadUsers(source): Promise<User[]> { // expect: explicit-boundary-types
  return [source];
}

export function logUsers(...parts): void { // expect: explicit-boundary-types
  void parts;
}

export const countUsers = (filter: string) => users.filter((user) => user.id === filter).length; // expect: explicit-boundary-types

export const describeUser = function (user): string { // expect: explicit-boundary-types
  return user.id;
};

function pageUsers(limit: number, since = Date.now()): User[] { // expect: explicit-boundary-types
  return users.slice(since, limit);
}

export { pageUsers };

export default function (input: string) { // expect: explicit-boundary-types
  return input.length;
}

export class UserService {
  constructor(repository) { // expect: explicit-boundary-types
    void repository;
  }

  find(id: string) { // expect: explicit-boundary-types
    return users.find((user) => user.id === id);
  }

  public static create() { // expect: explicit-boundary-types
    return new UserService([]);
  }

  get size() { // expect: explicit-boundary-types
    return users.length;
  }

  set label(value) { // expect: explicit-boundary-types
    void value;
  }

  handle = (event: string) => event.length; // expect: explicit-boundary-types
}

export namespace Users {
  export function first() { // expect: explicit-boundary-types
    return users[0];
  }
}
