// @path src/orders/order.spec.ts
// @alias @/*
import { Clock } from '@/clock';
import { tax } from '../tax';

export const total = tax(Clock.now());
