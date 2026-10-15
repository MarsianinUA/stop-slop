type Maybe<T> = T | null;

interface ApiKey {
  name: string;
  expiresAt: string | null;
  note?: string;
  revokedAt?: Date | undefined;
}

type Page = {
  cursor?: Maybe<string>;
  find?(id: string): string | null;
};

function update(name?: string | null): void {
  void name;
}

class Filter {
  status: string | null = null;
  label?: string;
}

class Draft {
  constructor(note?: string | null) {
    void note;
  }
}

export type { ApiKey, Page };
export { update, Filter, Draft };
