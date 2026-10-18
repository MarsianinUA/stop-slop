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
  private readonly cache = new Map<string, User>();
  private readonly logger: Logger;
  readonly #requests: number[] = [];
  private static readonly instances = new Set<string>();
  private retries = 3;
  private label = 'users';
  private mode = 'fast';
  private readonly kind = Kind.User;
  // `pick<T>` returns the literal type of its argument; a call without such an argument does not.
  private region = pick('north');
  private readonly started = Date.now();

  constructor(
    private readonly repo: Repo,
    @Inject('TOKEN') private readonly token: string,
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
  private color = 'blue';
  private size = 'large';
  private readonly weight = 'bold';

  paint(): string {
    const { color } = this;
    const size = this.size;
    return `${color} ${size} ${this.weight.length}`;
  }
}

export { UserService, Theme };
