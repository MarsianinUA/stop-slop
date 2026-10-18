// @alias @/*
import type { Account } from '@/accounts/account.type';
import type { Remote } from './remote';

interface Customer {
  name: string;
  email: string;
}

interface Order {
  id: string;
  total: number;
  customer: Customer;
}

export function orderLabel(order: Order): string { // expect: prefer-narrow-param
  return `#${order.id}`;
}

export function greet(customer: Customer): string { // expect: prefer-narrow-param
  const { name } = customer;
  return `Hello, ${name}`;
}

export const isLarge = (order: Order): boolean => order.total > 100; // expect: prefer-narrow-param

export function buyerName(order: Order): string { // expect: prefer-narrow-param
  return order.customer.name;
}

export function shipping(box: { weightKg: number; label: string }): number { // expect: prefer-narrow-param
  return box['weightKg'] * 2;
}

export class Mailer {
  send(customer: Customer, text: string): string { // expect: prefer-narrow-param
    const to = customer.email;
    return `${to}: ${text} (${customer?.email})`;
  }
}

export function endpoint(remote: Remote): string { // expect: prefer-narrow-param
  return remote.url;
}

export function accountKey(account: Account): string { // expect: prefer-narrow-param
  return account.key;
}
