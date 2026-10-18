class Counter {
  private count = 0;
  private readonly step: number = 1;
  protected base = 0;
  public total = 0;
  #last: number | null = null;
  private timer?: ReturnType<typeof setTimeout>;
  private items: string[] = [];
  private pending = new Map<string, number>();
  private static created = 0;
  private declare hidden: string;

  constructor(
    private readonly name: string,
    private other: Counter | null,
  ) {
    Counter.created++;
  }

  increment(): void {
    this.count += this.step;
    this.#last = Date.now();
    this.items = [...this.items, this.name];
  }

  start(): void {
    this.timer = setTimeout(() => this.increment(), 10);
  }

  copy(from: Counter): void {
    from.pending = new Map();
    if (this.other) {
      this.other = null;
    }
  }
}

class Lazy {
  private value: number;

  constructor() {
    setTimeout(() => {
      this.value = 1;
    });
  }
}

class Dynamic {
  private first = 1;

  set(key: 'first', value: number): void {
    this[key] = value;
  }
}

class Rows {
  private rows: string[] = [];
  private cursor = 0;

  reset(): void {
    [this.rows, this.cursor] = [[], 0];
  }
}

class TreeNode {
  private children: TreeNode[] = [];

  constructor(parent?: TreeNode) {
    if (parent) {
      parent.children = [...parent.children, this];
    }
  }
}

declare function Column(): PropertyDecorator;

class Entity {
  @Column() private name!: string;
  private loaded!: boolean;
}

declare class Ambient {
  private size: number;
}

class Draft {
  private note?: string = 'draft';

  constructor() {
    delete this.note;
  }
}

export { Counter, Lazy, Dynamic, Rows, TreeNode, Entity, Ambient, Draft, Cast };

class Cast {
  private second = 2;

  set(key: 'second', value: number): void {
    (this as Cast)[key] = value;
  }
}
