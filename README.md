# stop-slop

A filter for slop in TypeScript. It's my own linter. It checks the files of my branch against my conventions, then either fixes what it finds or turns it into review comments for a colleague's PR. An agent drives it through a skill, and a hook catches the agent's own slop right after each edit.

Why I wrote my own:

- I'm not the only one writing code. I want to leave PR comments fast, but checked first, not at random.
- The project has its lint config, and nobody will bend it to my taste. Fair enough, so this lives outside it.
- Clean up the campsite after yourself. It checks the whole files I touched, other people's mess in them too, and leaves every other file alone.
- I tried scripts in TS and `.mjs`. One core, one file at a time, painfully slow. This one is Rust on the [oxc](https://github.com/oxc-project/oxc) parser and checks files in parallel. A branch takes about 100 ms, a whole repo of 1,000 files about 130 ms.

## What it checks

103 rules: types, naming, functions and classes, data that wanders through functions for nothing, imports, config, errors and async, logs, style, NestJS, comments and tests.

- [All the rules](docs/rules.md)
- [How it works, how to install it, and what it skips on purpose](docs/spec.md)

I ported some rules from [anti-slop](https://github.com/dmmulroy/anti-slop). Its license is in [THIRD_PARTY.md](THIRD_PARTY.md).
