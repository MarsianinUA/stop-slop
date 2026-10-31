# Rules

All 116 rules stop-slop checks. `stop-slop rules` prints the same list, and each rule lives in its own folder under `rules/`.

- **error.** The rule is sure. After a fix, none of these may remain.
- **candidate.** A hunch. The program flags it, the agent decides if it's a real problem, and a false one goes to the ignore list with a reason.
- **agent.** The program can't see it, so the agent checks it by hand.
- **Fix.** A "yes" means the program fixes it itself, without changing what the code does.

## Types

How to write types and where to keep them.

| Rule | What it catches | Kind | Fix |
|---|---|---|---|
| [`explicit-boundary-types`](../rules/types/explicit-boundary-types/rule.toml) | An exported function or a public method without parameter or return types | error |  |
| [`exported-type-in-own-file`](../rules/types/exported-type-in-own-file/rule.toml) | An exported type, interface or enum in a file with runtime code | error |  |
| [`no-any`](../rules/types/no-any/rule.toml) | `any` anywhere | error |  |
| [`no-chained-type-assertions`](../rules/types/no-chained-type-assertions/rule.toml) | `value as unknown as T` | error |  |
| [`no-enum-raw-value`](../rules/types/no-enum-raw-value/rule.toml) | A raw string where the file has the enum member: `'DASHBOARD'` instead of `Source.Dashboard` | candidate |  |
| [`no-known-value-widening`](../rules/types/no-known-value-widening/rule.toml) | A known value put into `Record<string, X>`, `object` or `unknown` | error |  |
| [`no-non-null-assertion`](../rules/types/no-non-null-assertion/rule.toml) | `value!`, a non-null assertion | error |  |
| [`no-object-parameters`](../rules/types/no-object-parameters/rule.toml) | A parameter of type `object` | error |  |
| [`no-optional-and-null`](../rules/types/no-optional-and-null/rule.toml) | `field?: T \| null`: a field that is both optional and nullable | error |  |
| [`no-optional-in-emitted`](../rules/types/no-optional-in-emitted/rule.toml) | An optional `?:` field in a shape the service sends out: a response, DTO, view or row | candidate |  |
| [`no-own-type-reshaping`](../rules/types/no-own-type-reshaping/rule.toml) | `Omit`, `Pick`, `Partial` and other reshaping of a type of this project, or `Own & { ... }` in place | error |  |
| [`no-string-literal-union`](../rules/types/no-string-literal-union/rule.toml) | `type Field = 'sku' \| 'price'` instead of an enum | error |  |
| [`no-trivial-annotation`](../rules/types/no-trivial-annotation/rule.toml) | A type annotation the value already gives, like `const count: number = 5` or `const repo: Repo = new Repo()` | error | yes |
| [`no-type-in-constants-file`](../rules/types/no-type-in-constants-file/rule.toml) | A type or interface in a constants file | error |  |
| [`no-unexplained-as`](../rules/types/no-unexplained-as/rule.toml) | `value as T` without a `// SAFETY:` comment; `as const` is fine | error |  |
| [`no-unsafe-dictionary-type`](../rules/types/no-unsafe-dictionary-type/rule.toml) | A dictionary type with `unknown`, `any`, `object` or `{}` values, like `Record<string, unknown>` | error |  |
| [`no-widen-then-assert`](../rules/types/no-widen-then-assert/rule.toml) | A value widened to `unknown`, then narrowed back with `as` | error |  |
| [`prefer-nullish-coalescing`](../rules/types/prefer-nullish-coalescing/rule.toml) | `value \|\| default` where `??` keeps a real `0`, `''` or `false` | candidate |  |
| [`prefer-readonly`](../rules/types/prefer-readonly/rule.toml) | A private field that only the constructor sets, without `readonly` | error | yes |

## Naming

File, folder and symbol names.

| Rule | What it catches | Kind | Fix |
|---|---|---|---|
| [`acronym-case`](../rules/naming/acronym-case/rule.toml) | An acronym in capitals inside a name: `userID`, `loadHTTPURL` instead of `userId`, `loadHttpUrl` | error | yes |
| [`async-get-name`](../rules/naming/async-get-name/rule.toml) | An async `get*` that awaits outside work, where `fetch*` or `load*` names the boundary | candidate |  |
| [`boolean-prefix`](../rules/naming/boolean-prefix/rule.toml) | A boolean whose name does not start with `is`, `has`, `can` or `should` | error | yes |
| [`catch-param-name`](../rules/naming/catch-param-name/rule.toml) | A catch parameter not named `error`: `catch (err)`, `catch (e)` | error | yes |
| [`enum-member-case`](../rules/naming/enum-member-case/rule.toml) | An enum member not in PascalCase: `ETHEREUM` instead of `Ethereum` | error |  |
| [`file-name-pattern`](../rules/naming/file-name-pattern/rule.toml) | A file name that is not `<feature>.<role>.ts` in kebab-case | error |  |
| [`kebab-case-dirs`](../rules/naming/kebab-case-dirs/rule.toml) | A folder name that is not in kebab-case | error |  |
| [`naming-case`](../rules/naming/naming-case/rule.toml) | A name in the wrong case: PascalCase for types and classes, camelCase for values, CONSTANT_CASE for module constants | error | yes |
| [`no-interface-prefix`](../rules/naming/no-interface-prefix/rule.toml) | An interface named with an `I` prefix, like `IUser` | error | yes |
| [`no-negative-boolean`](../rules/naming/no-negative-boolean/rule.toml) | A negative boolean name: `isNotActive`, `hasNoItems` | error | yes |
| [`no-repeated-context`](../rules/naming/no-repeated-context/rule.toml) | An enum member that repeats its enum: `Status.StatusActive` | error |  |
| [`no-single-letter-name`](../rules/naming/no-single-letter-name/rule.toml) | A single-letter name outside a one-line callback and a loop counter | error |  |
| [`no-underscore-prefix`](../rules/naming/no-underscore-prefix/rule.toml) | A name with a `_` prefix, like `_value` | error | yes |
| [`no-vague-name`](../rules/naming/no-vague-name/rule.toml) | A vague name: `Manager`, `Base`, `Common`, `Helper` or `Util` in a type or function name, a variable called just `data`, `result`, `temp` or `item` | candidate |  |
| [`unit-in-name`](../rules/naming/unit-in-name/rule.toml) | A numeric `timeout`, `delay`, `ttl`, `interval` or `size` without a unit: `timeoutMs`, `sizeBytes` | candidate |  |

## Functions and classes

What goes in a class file and a functions file, parameters, `if` chains and `switch`, state.

| Rule | What it catches | Kind | Fix |
|---|---|---|---|
| [`class-member-order`](../rules/functions-classes/class-member-order/rule.toml) | Class members out of the order fields, constructor, methods | error | yes |
| [`derived-field`](../rules/functions-classes/derived-field/rule.toml) | A value computed from other data is stored as a second field and can drift from its source | agent |  |
| [`exhaustive-switch`](../rules/functions-classes/exhaustive-switch/rule.toml) | A `switch` over enum members or `.kind` without a `default` | error |  |
| [`inline-single-caller`](../rules/functions-classes/inline-single-caller/rule.toml) | A private function or method called once, as a step of its caller | candidate |  |
| [`max-positional-params`](../rules/functions-classes/max-positional-params/rule.toml) | More than two positional parameters | error |  |
| [`no-boolean-param`](../rules/functions-classes/no-boolean-param/rule.toml) | A boolean flag parameter: the function branches on it | error |  |
| [`no-class-in-function-file`](../rules/functions-classes/no-class-in-function-file/rule.toml) | A class in a functions file (`*.util.ts`, `*.mapper.ts`, `*.config.ts`, `*.schema.ts`) or a constants file | error |  |
| [`no-constant-in-class-file`](../rules/functions-classes/no-constant-in-class-file/rule.toml) | A constant at the top level of a file with a class | error |  |
| [`no-function-in-class-file`](../rules/functions-classes/no-function-in-class-file/rule.toml) | A function at the top level of a file with a class | error |  |
| [`no-if-else-cascade`](../rules/functions-classes/no-if-else-cascade/rule.toml) | Three or more `if` branches in a row that compare one value | candidate |  |
| [`no-instanceof-chain`](../rules/functions-classes/no-instanceof-chain/rule.toml) | A chain of `instanceof` checks on one value | candidate |  |
| [`no-manual-client`](../rules/functions-classes/no-manual-client/rule.toml) | A service, controller or job builds a client itself: `new RedisClient(..)`, `createClient(..)` | candidate |  |
| [`no-manual-singleton`](../rules/functions-classes/no-manual-singleton/rule.toml) | A hand-made singleton: a static field that keeps the class's own instance and a static method that hands it out (`static instance` + `getInstance()`) | error |  |
| [`no-module-state`](../rules/functions-classes/no-module-state/rule.toml) | Module state: a top-level `let` or `var`, or a top-level constant that code changes (`cache.set(..)`) | error |  |
| [`no-native-proxy`](../rules/functions-classes/no-native-proxy/rule.toml) | A native `Proxy`: `new Proxy(..)` or `Proxy.revocable(..)` | error |  |
| [`no-param-mutation`](../rules/functions-classes/no-param-mutation/rule.toml) | A function changes its parameter: `order.total = ..`, `items.push(..)` | candidate |  |
| [`one-responsibility`](../rules/functions-classes/one-responsibility/rule.toml) | A function does more than one thing | agent |  |
| [`options-defaults-in-signature`](../rules/functions-classes/options-defaults-in-signature/rule.toml) | An options key gets its default in the body (`options.x ?? 3`) instead of in the signature | candidate |  |

## Wandering data

Objects that travel through functions that barely use them.

| Rule | What it catches | Kind | Fix |
|---|---|---|---|
| [`no-pass-through-param`](../rules/wandering-data/no-pass-through-param/rule.toml) | A function only hands a parameter on, and the next function of the file hands it on again: `a(order)` -> `b(order)` -> `c(order)` | candidate |  |
| [`no-round-trip-object`](../rules/wandering-data/no-round-trip-object/rule.toml) | `order = applyX(order)`, where `applyX` returns its parameter or `{ ...order, x }` | error |  |
| [`prefer-narrow-param`](../rules/wandering-data/prefer-narrow-param/rule.toml) | A function takes an object but reads only one field of it | candidate |  |
| [`wandering-data-chain`](../rules/wandering-data/wandering-data-chain/rule.toml) | An object travels through functions in several files that do not read it, or that read only one field of it | agent |  |

## Imports

| Rule | What it catches | Kind | Fix |
|---|---|---|---|
| [`banned-packages`](../rules/imports/banned-packages/rule.toml) | An import of a package the platform replaces: `axios`, `node-fetch`, `got`, `uuid`, `dotenv`, `lodash` | error |  |
| [`import-order`](../rules/imports/import-order/rule.toml) | Imports out of the order Node built-ins, external packages, workspace packages and aliases, relative paths | error | yes |
| [`mixed-path-style`](../rules/imports/mixed-path-style/rule.toml) | One file imports both through a tsconfig alias (`@/...`) and through `../` | candidate |  |
| [`no-giant-barrel`](../rules/imports/no-giant-barrel/rule.toml) | An `index.ts` made only of `export * from` | candidate |  |
| [`no-undeclared-global`](../rules/imports/no-undeclared-global/rule.toml) | A Node global used without an import: `performance` without `node:perf_hooks` | error | yes |
| [`node-protocol`](../rules/imports/node-protocol/rule.toml) | `'fs'` instead of `'node:fs'` | error | yes |

## Config and constants

| Rule | What it catches | Kind | Fix |
|---|---|---|---|
| [`no-loose-constant`](../rules/config/no-loose-constant/rule.toml) | A literal constant loose at the top of a runtime file without a class | candidate |  |
| [`no-process-env`](../rules/config/no-process-env/rule.toml) | `process.env` read outside a `*.config.ts` file or an env helper | error |  |
| [`no-stand-special-case`](../rules/config/no-stand-special-case/rule.toml) | Code that mentions `/stand`, `STOP_SLOP`, `stand-guest`, `microsandbox` or `preload.cjs`, so the app behaves otherwise on the stand | error |  |

## Errors and async

| Rule | What it catches | Kind | Fix |
|---|---|---|---|
| [`bulk-all-settled`](../rules/errors-async/bulk-all-settled/rule.toml) | `Promise.all(items.map(..))` instead of `Promise.allSettled` | candidate |  |
| [`error-code-numeric`](../rules/errors-async/error-code-numeric/rule.toml) | A member of an `*ErrorCode` enum or object that is not an explicit number | error |  |
| [`error-extends-error`](../rules/errors-async/error-extends-error/rule.toml) | A class named `*Error` that does not extend `Error` | error |  |
| [`no-empty-catch`](../rules/errors-async/no-empty-catch/rule.toml) | An empty `catch`, or a `catch` that only logs | error |  |
| [`no-floating-promise`](../rules/errors-async/no-floating-promise/rule.toml) | A call of an async function of this file without `await`, `return` or `void` | error |  |
| [`no-inline-throw-new`](../rules/errors-async/no-inline-throw-new/rule.toml) | `throw new X(..)` outside an error factory in `errors/` | candidate |  |
| [`no-then-chain`](../rules/errors-async/no-then-chain/rule.toml) | `.then(..)` instead of `await` | error |  |
| [`return-await-in-try`](../rules/errors-async/return-await-in-try/rule.toml) | `return call()` without `await` inside `try` of an async function | candidate |  |

## Logs

| Rule | What it catches | Kind | Fix |
|---|---|---|---|
| [`no-console`](../rules/logs/no-console/rule.toml) | `console.*` in application code | error |  |
| [`no-secret-in-log`](../rules/logs/no-secret-in-log/rule.toml) | A value named like a secret (`token`, `password`, `authorization`, `apiKey`...) goes into a log call | candidate |  |

## Style

| Rule | What it catches | Kind | Fix |
|---|---|---|---|
| [`arrow-callback`](../rules/style/arrow-callback/rule.toml) | A `function () {}` callback that does not use its own `this` | error | yes |
| [`max-condition-clauses`](../rules/style/max-condition-clauses/rule.toml) | A condition of three or more clauses, like `a && b && c` | error |  |
| [`max-nesting`](../rules/style/max-nesting/rule.toml) | More than 4 levels of nesting | error |  |
| [`newspaper-layout`](../rules/style/newspaper-layout/rule.toml) | A function or method declared above the code that calls it | candidate |  |
| [`no-array-filter-map`](../rules/style/no-array-filter-map/rule.toml) | `.filter(..).map(..)` or `.map(..).filter(..)` in a row on an array | error |  |
| [`no-bare-block`](../rules/style/no-bare-block/rule.toml) | A bare `{ }` block that only limits the scope of its variables | error |  |
| [`no-conditional-empty-object-spread`](../rules/style/no-conditional-empty-object-spread/rule.toml) | A spread of a `?:` with an empty object branch: `...(x ? { x } : {})` | error |  |
| [`no-nested-ternary`](../rules/style/no-nested-ternary/rule.toml) | A ternary inside a ternary | error |  |
| [`no-reduce-accumulator-copy`](../rules/style/no-reduce-accumulator-copy/rule.toml) | A `reduce` callback copies its accumulator: `{ ...acc }`, `Object.assign({}, acc)`, `acc.concat(..)` | error |  |
| [`no-reflect-apply`](../rules/style/no-reflect-apply/rule.toml) | `Reflect.apply(..)` instead of a typed call | error |  |
| [`no-reflect-get`](../rules/style/no-reflect-get/rule.toml) | `Reflect.get(..)` instead of a typed property read | error |  |
| [`no-top-level-side-effect`](../rules/style/no-top-level-side-effect/rule.toml) | A call or other work at the top level of a module that exports something, outside `main.ts` and bootstrap files | candidate |  |
| [`no-var`](../rules/style/no-var/rule.toml) | `var` | error |  |
| [`prefer-const`](../rules/style/prefer-const/rule.toml) | A `let` that is never reassigned | error | yes |

## NestJS

On only where `@nestjs/core` is a dependency.

| Rule | What it catches | Kind | Fix |
|---|---|---|---|
| [`config-get-or-throw`](../rules/nestjs/config-get-or-throw/rule.toml) | `configService.get(..)` instead of `getOrThrow(..)` | error |  |
| [`module-exports-needed`](../rules/nestjs/module-exports-needed/rule.toml) | A Nest module exports a provider that no other module injects | agent |  |
| [`nest-http-code`](../rules/nestjs/nest-http-code/rule.toml) | A Nest route handler without `@HttpCode(..)` | error | yes |
| [`no-api-property-optional`](../rules/nestjs/no-api-property-optional/rule.toml) | `@ApiPropertyOptional` on a field of a response DTO | error |  |
| [`no-inline-inject-token`](../rules/nestjs/no-inline-inject-token/rule.toml) | `@Inject('TOKEN')` with a string instead of a token constant | error |  |
| [`no-redeclared-framework-enum`](../rules/nestjs/no-redeclared-framework-enum/rule.toml) | An own `HttpStatus` instead of the one Nest exports | error |  |
| [`no-request-scope`](../rules/nestjs/no-request-scope/rule.toml) | A provider in `Scope.REQUEST` | error |  |
| [`route-kebab-case`](../rules/nestjs/route-kebab-case/rule.toml) | A route segment of a Nest controller or handler not in kebab-case | error |  |

## Comments

| Rule | What it catches | Kind | Fix |
|---|---|---|---|
| [`comment-restates-code`](../rules/comments/comment-restates-code/rule.toml) | A comment retells the code. Step headers in a long function (`// --- Reconcile.`) are fine | agent |  |
| [`comment-sentence`](../rules/comments/comment-sentence/rule.toml) | A comment is not a finished sentence: it starts with a lowercase letter or does not end with a period. Directives, `TODO` and commented-out code are left alone | error |  |
| [`no-commented-out-code`](../rules/comments/no-commented-out-code/rule.toml) | A comment that parses as code | candidate |  |
| [`todo-owner`](../rules/comments/todo-owner/rule.toml) | `TODO` without an owner | error |  |

## Tests

Test files only.

| Rule | What it catches | Kind | Fix |
|---|---|---|---|
| [`no-internal-module-mock`](../rules/tests/no-internal-module-mock/rule.toml) | `jest.mock('./..')` of a module of this project; packages may be mocked | error |  |
| [`test-name`](../rules/tests/test-name/rule.toml) | A test name that says nothing: `it('works')`, `test1` | error |  |
| [`test-quality`](../rules/tests/test-quality/rule.toml) | A test that detects changes, tests a mock or repeats the code | agent |  |

## Performance

Code that costs more on Node than it looks. Off until `enabled` in `config.toml` names the group.

| Rule | What it catches | Kind | Fix |
|---|---|---|---|
| [`no-async-foreach`](../rules/performance/no-async-foreach/rule.toml) | `.forEach(async ..)`: nothing waits for the callbacks | error |  |
| [`no-await-in-loop`](../rules/performance/no-await-in-loop/rule.toml) | `await` in the body of a loop, except `for await` | candidate |  |
| [`no-copy-in-loop`](../rules/performance/no-copy-in-loop/rule.toml) | A variable copied into itself in a loop or a listener: `all = [...all, item]`, `buffer = Buffer.concat([buffer, chunk])` | error |  |
| [`no-delete-property`](../rules/performance/no-delete-property/rule.toml) | `delete user.password` on an object with a fixed set of keys, not a dictionary | candidate |  |
| [`no-lookup-in-loop`](../rules/performance/no-lookup-in-loop/rule.toml) | A search of an array in a loop over another collection: `ids.includes(order.id)` inside `for (const order of orders)` | candidate |  |
| [`no-nested-quantifier`](../rules/performance/no-nested-quantifier/rule.toml) | A regex that repeats a group of one repeated character, with more to match after it: `^(a+)+$`, `([a-z]+-?)+@` | error |  |
| [`no-recursive-next-tick`](../rules/performance/no-recursive-next-tick/rule.toml) | A function that passes itself to `process.nextTick` | error |  |
| [`no-shift-in-loop`](../rules/performance/no-shift-in-loop/rule.toml) | `shift()` or `unshift()` of the array a loop goes through: `while (queue.length) queue.shift()` | candidate |  |
| [`no-sync-in-handler`](../rules/performance/no-sync-in-handler/rule.toml) | `readFileSync`, `execSync`, `bcrypt.hashSync` and other blocking calls in a Nest handler or provider, outside the constructor and the startup hooks | candidate |  |

## Branching

Tests that decide nothing and functions too tangled to follow. Off until `enabled` in `config.toml` names the group.

| Rule | What it catches | Kind | Fix |
|---|---|---|---|
| [`max-cognitive-complexity`](../rules/branching/max-cognitive-complexity/rule.toml) | A function with a cognitive complexity above 15, counted as SonarJS counts it | error |  |
| [`no-identical-branches`](../rules/branching/no-identical-branches/rule.toml) | Branches that are the same code: an `if` and its `else`, an `if` and the `else if` after it, the two sides of `?:`, adjacent `case`s | error |  |
| [`no-implied-condition`](../rules/branching/no-implied-condition/rule.toml) | A test an outer `if`, `?:`, `&&` or `\|\|` already decided, on a `const` or a parameter nothing reassigns: `if (user) { if (user) .. }` | error |  |
