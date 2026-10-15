interface User { id: string }
declare class Logger {
  constructor(name: string);
}
declare class Repo {}
declare function Inject(token: string): ParameterDecorator;
enum Kind {
  User = 'user',
}

class UserService {
  private readonly cache = new Map<string, User>();
  private readonly logger: Logger;
  readonly #requests: number[] = [];
  private static readonly instances = new Set<string>();
  private readonly retries = 3;
  private label = 'users';
  private mode = 'fast';
  private readonly kind = Kind.User;

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
    return this.mode === 'slow' ? undefined : this.cache.get(id);
  }
}

export { UserService };
