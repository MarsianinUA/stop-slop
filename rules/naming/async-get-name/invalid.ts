declare function query(sql: string): Promise<string[]>;

export async function getOrders(): Promise<string[]> { // expect: async-get-name
  return await query('select id from orders');
}

export const getCustomers = async (): Promise<string[]> => { // expect: async-get-name
  const rows = await query('select id from customers');
  return rows;
};

export class OrderService {
  async getOrder(id: string): Promise<string | null> { // expect: async-get-name
    const [row] = await query(`select id from orders where id = '${id}'`);
    return row ?? null;
  }

  private async getTotals(): Promise<number[]> { // expect: async-get-name
    const totals: number[] = [];
    for await (const row of [query('select total from orders')]) {
      totals.push(row.length);
    }
    return totals;
  }

  report(): Promise<number[]> {
    return this.getTotals();
  }
}

export async function getInvoices(id: string): Promise<string[]>; // expect: async-get-name
export async function getInvoices(id: string, all: boolean): Promise<string[]>;
export async function getInvoices(id: string, all?: boolean): Promise<string[]> {
  return await query(`select id from invoices where id = '${id}' and ${all ?? false}`);
}

export const getRefunds = async function getRefunds(): Promise<string[]> { // expect: async-get-name
  return await query('select id from refunds');
};
