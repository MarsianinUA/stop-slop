declare function Injectable(): ClassDecorator;

@Injectable()
export class OrderService {
  static readonly LIMIT = 10;
  private readonly cache = new Map<string, number>();
  private readonly onEvent = (name: string): number => this.cache.get(name) ?? 0;

  constructor(private readonly prefix: string) {}

  static create(): OrderService {
    return new OrderService('orders');
  }

  get size(): number {
    return this.cache.size + OrderService.LIMIT;
  }

  read(name: string): number {
    return this.onEvent(`${this.prefix}:${name}`);
  }
}

export abstract class Repository {
  protected abstract readonly table: string;

  abstract find(id: string): string;
}

export const Anonymous = class {
  value = 1;

  read(): number {
    return this.value;
  }
};

declare class Legacy {
  run(): void;
  value: number;
}

export const legacy: Legacy | null = null;

export const handlers = {
  run(): number {
    return 1;
  },
  value: 2,
};
