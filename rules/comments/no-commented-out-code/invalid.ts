// import { readFile } from 'node:fs/promises'; // expect: no-commented-out-code
import { Get, Param } from '@nestjs/common';

export function total(prices: number[], rate: number): number {
  // const tax = prices.length * rate; // expect: no-commented-out-code
  const sum = prices.reduce((all, price) => all + price, 0);
  // if (prices.length === 0) { // expect: no-commented-out-code
  //   return 0;
  // }
  // The old rounding, kept for a while.
  // return Math.round(sum); // expect: no-commented-out-code
  return sum * (1 + rate);
}

export function first(prices: number[]): number {
  // const head = prices[0]; // expect: no-commented-out-code

  // logger.debug(head)
  return prices[0] ?? 0;
}

export class PriceController {
  // private readonly cache = new Map<string, number>(); // expect: no-commented-out-code
  private readonly pages = new Map<string, string>();

  // @Get(':id') // expect: no-commented-out-code
  // find(@Param('id') id: string) {
  //   return this.pages.get(id);
  // }

  @Get()
  list(@Param('page') page: string): string {
    return page; // return this.pages.size; // expect: no-commented-out-code
  }
}

export const options = {
  timeout: 5000,
  // retries: 3, // expect: no-commented-out-code
};

export interface Label {
  name: string;
  //   color?: string; // expect: no-commented-out-code
}

/* await refresh(options); */ // expect: no-commented-out-code
