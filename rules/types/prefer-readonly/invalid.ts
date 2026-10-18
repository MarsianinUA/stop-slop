interface User { id: string }
declare class Logger {
  constructor(name: string);
}
declare class Repo {}
declare function Inject(token: string): ParameterDecorator;
declare function pick<T>(value: T): T;
enum Kind {
  User = 'user',
}

class UserService {
  private cache = new Map<string, User>(); // expect: prefer-readonly
  private logger: Logger; // expect: prefer-readonly
  #requests: number[] = []; // expect: prefer-readonly
  private static instances = new Set<string>(); // expect: prefer-readonly
  private retries = 3; // expect: prefer-readonly
  private label = 'users'; // expect: prefer-readonly
  private mode = 'fast'; // expect: prefer-readonly
  private kind = Kind.User; // expect: prefer-readonly
  // `pick<T>` returns the literal type of its argument; a call without such an argument does not.
  private region = pick('north'); // expect: prefer-readonly
  private started = Date.now(); // expect: prefer-readonly

  constructor(
    private repo: Repo, // expect: prefer-readonly
    @Inject('TOKEN') private token: string, // expect: prefer-readonly
    name?: string,
  ) {
    this.logger = new Logger('users');
    if (name) {
      this.label = name;
    }
  }

  find(id: string): User | undefined {
    this.#requests.push(this.retries);
    void this.kind;
    if (this.region === 'south' || this.started < 0) {
      return undefined;
    }
    return this.mode === 'slow' ? undefined : this.cache.get(id);
  }
}

class Theme {
  private color = 'blue'; // expect: prefer-readonly
  private size = 'large'; // expect: prefer-readonly
  private weight = 'bold'; // expect: prefer-readonly

  paint(): string {
    const { color } = this;
    const size = this.size;
    return `${color} ${size} ${this.weight.length}`;
  }
}

export { UserService, Theme };
