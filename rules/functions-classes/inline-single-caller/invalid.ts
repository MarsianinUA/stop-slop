declare function query(sql: string): Promise<string[]>;
declare function audit(rows: string[]): void;

async function loadRows(table: string): Promise<string[]> { // expect: inline-single-caller
  return query(`select * from ${table}`);
}

function auditRows(rows: string[]): void { // expect: inline-single-caller
  audit(rows);
}

export async function report(table: string): Promise<number> {
  const rows = await loadRows(table);
  auditRows(rows);
  return rows.length;
}

export class Orders {
  async sync(): Promise<number> {
    await this.prepare();
    return this.#finish();
  }

  private async prepare(): Promise<void> { // expect: inline-single-caller
    await query('begin');
  }

  #finish(): number { // expect: inline-single-caller
    return 1;
  }
}

export function outer(): number {
  const inner = (): number => 2; // expect: inline-single-caller
  return inner();
}
