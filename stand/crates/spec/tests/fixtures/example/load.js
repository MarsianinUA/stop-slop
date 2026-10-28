const base = process.argv[2];
for (let i = 0; i < 100; i++) await fetch(`${base}/orders`, { method: "POST", body: "{}" });
