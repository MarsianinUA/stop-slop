const orders = require('./dist/orders.js');
const rows = [{ price: 2, qty: 3 }, { price: 5, qty: 1 }];
orders.total(rows);
orders.report(rows);
setTimeout(() => {
  orders.total(rows);
  orders.fib(5);
}, 2300);
