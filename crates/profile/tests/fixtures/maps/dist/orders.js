"use strict";
var __decorate = (this && this.__decorate) || function (decorators, target, key, desc) {
    var c = arguments.length, r = c < 3 ? target : desc === null ? desc = Object.getOwnPropertyDescriptor(target, key) : desc, d;
    if (typeof Reflect === "object" && typeof Reflect.decorate === "function") r = Reflect.decorate(decorators, target, key, desc);
    else for (var i = decorators.length - 1; i >= 0; i--) if (d = decorators[i]) r = (c < 3 ? d(r) : c > 3 ? d(target, key, r) : d(target, key)) || r;
    return c > 3 && r && Object.defineProperty(target, key, r), r;
};
Object.defineProperty(exports, "__esModule", { value: true });
exports.Orders = void 0;
exports.total = total;
exports.report = report;
exports.fib = fib;
function logged(target, key) {
    console.log(`decorated ${key}`);
}
class Orders {
    list() {
        return [];
    }
}
exports.Orders = Orders;
__decorate([
    logged
], Orders.prototype, "list", null);
function total(rows) {
    let sum = 0;
    for (const row of rows) {
        sum += row.price * row.qty;
    }
    return sum;
}
function report(rows) {
    return `total ${total(rows)}`;
}
function fib(n) {
    return n < 2 ? n : fib(n - 1) + fib(n - 2);
}
//# sourceMappingURL=orders.js.map