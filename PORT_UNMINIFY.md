# Port Analysis: `unminify` and `transpile`

## Executive summary

This report audits exactly the upstream subsystem under `/tmp/upstream-webcrack/packages/webcrack/src/unminify` and `/tmp/upstream-webcrack/packages/webcrack/src/transpile`. The audit covers **33 non-test TypeScript files** and **30 relevant test files** containing **150 test declarations**, including **6 skipped** and **2 todo** cases.

The subsystem is a synchronous Babel AST rewrite layer. It does not execute user JavaScript, does not use `isolated-vm`, and does not implement bundle extraction. `unminify` contains 23 safe transforms. `transpile` contains five safe transforms and one explicitly unsafe template-literal transform. The two aggregate exports are merged transforms; upstream invokes them in the main `webcrack` pipeline as `transpile` followed by `unminify` after preparation and optional deobfuscation.

The target `/home/ubuntu/webscrack` currently has Oxc parsing, code generation, minification, bookmarklet stripping, and only four textual substitutions in `unminify_source`: `!0`, `!1`, `void 0`, and `typeof undefined`. That implementation is not AST-safe and does not cover the audited behavior. A full port is feasible with Oxc, but it is a substantial visitor and scope-analysis implementation rather than a small patch. The recommended architecture is a sequence of explicit `VisitMut` passes over Oxc AST, with semantic analysis rebuilt before scope-sensitive groups and a conservative restricted constant evaluator for numeric and JSON rewrites.

## Audit scope and exports

### Files and public surface

The upstream file inventory is:

| Area | Export/index files | Transform files | Test files | Aggregate export |
|---|---:|---:|---:|---|
| `unminify` | 2 | 23 | 24 | `default export mergeTransforms({ name: 'unminify', tags: ['safe'], transforms: Object.values(transforms) })` |
| `transpile` | 2 | 6 | 6 | `default export mergeTransforms({ name: 'transpile', tags: ['safe'], transforms: Object.values(transforms) })` |
| **Total** | **4** | **28** | **30** | — |

`unminify/transforms/index.ts` exports, in source order, `blockStatements`, `computedProperties`, `forToWhile`, `infinity`, `invertBooleanLogic`, `jsonParse`, `logicalToIf`, `mergeElseIf`, `mergeStrings`, `numberExpressions`, `rawLiterals`, `removeDoubleNot`, `sequence`, `splitForLoopVars`, `splitVariableDeclarations`, `stringLiteralInTemplate`, `ternaryToIf`, `truncateNumberLiteral`, `typeofUndefined`, `unaryExpressions`, `unminifyBooleans`, `voidToUndefined`, and `yoda`.

`transpile/transforms/index.ts` exports `defaultParameters`, `logicalAssignments`, `nullishCoalescing`, `nullishCoalescingAssignment`, `optionalChaining`, and `templateLiterals`.

The aggregate transform itself is not a second fixed-point engine. `mergeTransforms` merges visitors into one Babel traversal. The order of `Object.values(transforms)` follows the export object order, but individual visitors use enter/exit traversal and may see changes made by earlier visitors during the same walk. The main pipeline separately runs preparation transforms (`block-statements`, `sequence`, and `split-variable-declarations`), then runs `transpile` and `unminify` together when the `unminify` option is enabled.

### Main upstream pipeline context

The relevant upstream `webcrack` stages are:

1. Parse with Babel using `sourceType: 'unambiguous'`, `allowReturnOutsideFunction: true`, `errorRecovery: true`, and JSX syntax.
2. Remove Babel node metadata, then apply preparation transforms: block statements, sequence expansion, and variable declaration splitting.
3. Optionally deobfuscate with the supplied sandbox.
4. If `options.unminify` is true, apply `[transpile, unminify]`.
5. Optionally mangle and perform later deobfuscation/JSX passes.
6. Generate output before unpacking.

The target Rust code currently invokes its textual `unminify_source` in `transform`, `format`, `minify`, and conditionally in `webcrack`. The target `webcrack` options include `unminify`, `unpack`, `deobfuscate`, and `mangle`, but the audited AST passes are not yet present.

## Behavior catalog: `unminify`

All 23 unminify transforms are tagged `safe` upstream. “Safe” means the project intends a semantics-preserving readability rewrite; it does not mean every transformation is valid for every hostile or unusual JavaScript construct. The scope-sensitive transforms are marked below.

| Transform | Matching behavior | Important edge cases and exclusions | Scope |
|---|---|---|---|
| `block-statements` | Wraps non-block `if` consequents/alternates and loop bodies in blocks. Converts an arrow body that is a sequence expression into a block containing `return sequence`. | Empty statements are left unwrapped. Only `if`, Babel `Loop`, and arrows are handled. | No |
| `computed-properties` | Converts computed string identifier names such as `obj['x']`, `obj?.['x']`, `{['x']: v}`, and `class C { ['m']() {} }` to non-computed identifier forms. | Uses Babel identifier-name validation. Does not rewrite object `['__proto__']` or class `['constructor']`, because those have special semantics. Invalid names such as `['1']` remain computed. | No |
| `for-to-while` | Converts `for (;;)` to `while (true)` and `for (; test;)` to `while (test)`. | Requires no initializer and no update. A `for` with either is unchanged. | No |
| `infinity` | Converts `1 / 0` to `Infinity` and `-1 / 0` to `-Infinity`. | Only exact numeric literal forms match. It refuses the rewrite when a lexical binding named `Infinity` exists in scope. | **Yes** |
| `invert-boolean-logic` | Converts `!(a == b)`, `!(a === b)`, `!(a != b)`, and `!(a !== b)` by swapping equality operators. Applies De Morgan to `&&`/`||`, recursively over the left-associated logical tree. | Does not invert nullish coalescing. A mixed expression such as `!((a ?? b) || c)` becomes `!(a ?? b) && !c`, not an invalid inversion of `??`. | No |
| `json-parse` | Replaces a call exactly shaped like global `JSON.parse(<string literal>)` with a Babel-parsed JavaScript expression after validating with host `JSON.parse`. | Invalid JSON is ignored. A local binding named `JSON` suppresses the rewrite. Parsing is expression parsing, so JSON object/array/scalar output becomes a normal AST literal. Large numeric JSON is accepted and emitted as a JavaScript number representation. | **Yes** |
| `logical-to-if` | Converts expression statements `left && right` to `if (left) { right; }`; converts `left || right` to `if (!left) { right; }`. | It only handles expression statements. Chained expressions preserve the left grouped test, e.g. `x && y && z()` becomes `if (x && y) { z(); }`. | No |
| `merge-else-if` | Converts `if (x) {} else { if (y) {} }` to `if (x) {} else if (y) {}`. | The alternate must be a block containing exactly one `if`; extra statements prevent the rewrite. | No |
| `merge-strings` | Folds adjacent string concatenation, including both sides of a variable: `"a" + "b" + xyz + "c"` becomes `"ab" + xyz + "c"`. | Only string literals are folded. It mutates the captured literal and clears the right literal to avoid repeated traversal concatenation. | No |
| `number-expressions` | Uses Babel `path.evaluate()` to fold a restricted matcher: unary minus of a string/number, and binary `+ - / % * ** & | >> >>> << ^` where operands are string/number or negative numeric literals. | Division is folded only when the result is an integer; decimal divisions remain source expressions. Babel evaluation and `valueToNode` determine coercion and literal emission. | No |
| `raw-literals` | Clears Babel `extra` metadata on string and numeric literals so the generator does not preserve minified/raw spellings. | It changes no semantic value. This is metadata behavior and must be mapped carefully to Oxc raw/span fields. | No |
| `remove-double-not` | Rewrites `!!x` in conditional tests to `x`; `!!!x` to `!x`; and callback bodies of literal-array methods `filter`, `find`, `findLast`, `findIndex`, `findLastIndex`, `some`, and `every` from `!!x` to `x`. | It targets exact matcher shapes. The array matcher requires a literal array receiver and an arrow callback. | No |
| `sequence` | Expands sequences in expression/return statements and in tests or operands of `if`, `switch`, `throw`, `for-in`, and `for-of`. It splits safe assignment RHS sequences when the assignment target is an identifier or a member with a simple identifier/safe-literal base/property. It handles selected `for` initializer/update cases and single-declarator variable initializers. It collapses an all-safe-literal sequence to its last expression. | Evaluation order is the central guard. It deliberately avoids `a[x()] = (b(), c())` because evaluating the member key before `b()` could change behavior. It does not rewrite short-circuit assignments (`||=`, `&&=`, `??=`). A `for` update sequence is expanded into a block only when the loop body is empty. | No |
| `split-for-loop-vars` | Pulls leading `var` declarators out of a `for` initializer when they are not referenced or assigned in the test/update. Leaves the remaining declarations in the loop. | Only `var` is eligible. The pass stops at the first binding used by test/update. `let` is intentionally ignored to preserve per-iteration binding semantics. | **Yes** |
| `split-variable-declarations` | Splits multi-declarator declarations into one declaration each; handles normal blocks, exported declarations, and `for (var ...;;)` with no test/update. A second visitor handles declarations encountered outside the block fast path. | A `for` initializer with `let`, or any initializer with a test/update, is not split. Multi-declarator loop bodies can still be split as ordinary block statements. | No |
| `string-literal-in-template-literal` | Inlines string expressions inside template literals, e.g. `` `Hello ${'World'}!` `` to `` `Hello World!` ``, merging adjacent quasis. | Escapes backslash, backtick, dollar, and control characters. Newline is intentionally not escaped in the helper. | No |
| `ternary-to-if` | Converts a conditional expression used as an expression statement to an `if/else` statement, and a returned conditional to an `if/else` containing separate returns. | Does not rewrite a conditional assigned to a variable or used in another expression. | No |
| `truncate-number-literal` | For bitwise operators `| & ^ << >> >>>`, truncates a numeric literal operand to JavaScript bitwise width. Shift-right/left literal RHS uses `31`; other cases use `0xffffffff`, then JavaScript bitwise conversion. | Only numeric literal operands are changed. The test verifies float truncation, overflow to `-1`, and shift `64` to `0`; a non-literal shift operand is unchanged. | No |
| `typeof-undefined` | Converts `typeof x > "u"` to `typeof x === "undefined"` and `typeof x < "u"` to `typeof x !== "undefined"`. | Exact string literal `"u"` and exact relational operator are required. | No |
| `unary-expressions` | In an expression statement, removes a leading `void`, `!`, or `typeof` and keeps the argument. Converts `return void x` into `x; return;`. | This intentionally favors deminification over all unusual runtime distinctions. In particular, removing `typeof` from `typeof undeclared` can expose a `ReferenceError`; the upstream test suite treats the pattern as safe. | No |
| `unminify-booleans` | Converts `!0`, `!!1`, and `!![]` to `true`; converts `!1` and `![]` to `false`. | Exact literal shapes only. It does not perform general truthiness evaluation. | No |
| `void-to-undefined` | Converts `void 0` to the identifier `undefined`. | Requires no local binding named `undefined`; a shadowing declaration preserves `void 0`. | **Yes** |
| `yoda` | Flips comparisons and selected commutative operators when the left operand is a pure value and the right operand is not. Operators are `==`, `===`, `!=`, `!==`, `<`, `>`, `<=`, `>=`, `*`, `^`, `&`, and `|`. | Pure values include strings, numbers, booleans, null, `undefined`, `NaN`, `Infinity`, and negative numbers/Infinity. It does not flip `+` or `-`; it leaves both-pure expressions and literal-right expressions unchanged. Relational operators are remapped when operands are swapped. | No |

### Notable sequencing and ordering interactions

Preparation applies `block-statements`, `sequence`, and `split-variable-declarations` together before the aggregate unminify pass. This matters because later transforms assume normalized blocks and separated declarations. For example, `sequence` can turn a sequence-valued statement into several statements, after which `ternary-to-if`, `logical-to-if`, and `merge-else-if` can see statement-level shapes. The Rust port should preserve this stage boundary rather than flattening all transforms into an arbitrary pass list.

The aggregate unminify transform has scope enabled because `infinity`, `json-parse`, `split-for-loop-vars`, and `void-to-undefined` need bindings. Babel merges all visitors into one traversal. A Rust implementation may use separate traversals for clarity, but must preserve the effective order and must not reuse stale scope information after structural edits.

## Behavior catalog: `transpile`

The transpile transforms reverse common compiler lowerings back into modern syntax. Five are tagged `safe`; `template-literals` is tagged `unsafe` because converting concatenation to template syntax can change coercion or overridden method behavior in edge cases.

| Transform | Matching behavior | Edge cases, tested gaps, and safety notes |
|---|---|---|
| `default-parameters` | Detects compiler-generated first statements in a function body and moves them into parameter assignment patterns. It recognizes conditional `arguments.length` checks, boolean-special forms, plain `arguments.length > index ? arguments[index] : undefined`, and a loose first-statement `if (x === undefined) { x = default; }`. Supports identifier, array-pattern, and object-pattern parameters. | The declaration/`if` must be at the start of the function body. A gap before a later parameter creates generated names such as `_param`, `_param2`, and `_param3`. The transform verifies that the loose binding is an actual function parameter. It uses Babel scope UID generation. |
| `logical-assignments` | Converts `x || (x = y)` and `x && (x = y)` to `x ||= y` and `x &&= y`. Handles temporary-variable forms for member, computed member, and doubly computed members, after verifying each temporary is a one-assignment variable. | It removes only verified temporary declarations. The `isTemporaryVariable` predicate requires exact reference count, one constant violation, and an uninitialized `var` declarator. |
| `nullish-coalescing` | Converts Babel temporary forms `(_a = a) !== null && _a !== undefined ? _a : b`, loose `(_a = a) != null ? _a : b`, and direct TS/SWC forms to `a ?? b`. It also handles a parameter-IIFE form used in default parameters. | Member-expression esbuild form and flipped forms are explicitly TODO. The transform must not rewrite arbitrary repeated expressions unless the matcher proves the generated temporary preserves evaluation count. |
| `nullish-coalescing-assignment` | Converts `a ?? (a = b)` to `a ??= b`, and temporary member forms such as `(_a = a).b ?? (_a.b = c)` or computed equivalents to `a.b ??= c`. | Several deeper TypeScript/computed cases are skipped in tests. Only the implemented matcher shapes should be ported initially. |
| `optional-chaining` | Converts direct TS form `a === null || a === undefined ? undefined : a.b` to `a?.b`, including computed `a?.[b]`; also converts the Babel temporary member form after verifying its temporary. | Call-expression forms (`a?.()`) and Babel computed/call forms are skipped and are not implemented by this transform. The port should not claim full optional-chaining lowering reversal until those cases are separately designed. |
| `template-literals` | Converts literal `.concat()` chains to template literals; merges template literals with `+` on either side; folds string literal pieces into quasis and carries nested template expressions through. | Tagged `unsafe`. Escapes backslash, backtick, dollar, NUL, backspace, form feed, carriage return, tab, and vertical tab, but deliberately leaves newline unescaped. The source contains a TODO for Babel's `ignoreToPrimitiveHint` option, which would use `+` instead of `.concat`. |

The tests cover default-parameter patterns, logical assignment patterns, nullish coalescing and assignment patterns, member optional chaining, and template concatenation. The six skipped tests are concentrated in deeper computed/call optional chaining and TypeScript temporary patterns. Two nullish tests are TODO, including an esbuild member form and a flipped conditional.

## Babel, matcher, and runtime dependencies

### Direct AST dependencies

The audited files depend on the following Babel packages:

| Dependency | Usage in this subsystem |
|---|---|
| `@babel/types` | AST node predicates and constructors for every structural transform, including optional members, assignment patterns, blocks, returns, literals, template quasis, and logical assignments. |
| `@babel/traverse` | `NodePath` typing in `remove-double-not`; traversal, scope, binding references, path replacement/removal, insertion, and generated UIDs are supplied by the shared `ast-utils` layer. |
| `@babel/parser` | `json-parse` calls `parseExpression` after host JSON validation. |
| `@babel/template` | `logical-to-if` and `ternary-to-if` build statement templates. |
| `@babel/helper-validator-identifier` | `computed-properties` checks whether a string is a valid identifier name. |
| `@codemod/matchers` | Declarative structural matching and capture variables in nearly every transform. |

The shared transform engine (`ast-utils/transform.ts`) merges visitors, toggles Babel `noScope` based on transform metadata, tracks `changes`, and exposes `isTemporaryVariable`. The subsystem does not directly import `@babel/generator`, but the shared test harness and surrounding generator do; generated snapshots therefore include Babel's formatting and literal choices.

### `isolated-vm` dependency

There is **no `isolated-vm` import in either audited directory**. Upstream declares optional dependencies `isolated-vm-6` and `isolated-vm-7` at the package level for the separate deobfuscation sandbox. The `sandbox` option and runtime-assisted decoding belong to the broader `webcrack` pipeline, not to these synchronous AST transforms. `json-parse` uses JavaScript host `JSON.parse` only to validate a string and does not evaluate arbitrary input. A Rust port of `unminify`/`transpile` should not add an embedded JavaScript runtime or isolated VM.

### Security and semantic boundary

The transform layer is source-to-source rewriting. It does not execute expressions such as `1 + 1` in a VM; `number-expressions` uses Babel's static evaluator for matched constant forms, and `json-parse` parses data as an AST expression. The port should preserve this boundary. Any future general evaluator or deobfuscator must remain a separate feature with an explicit sandbox design.

## Current target state and gaps

The target repository currently depends directly on `oxc_allocator`, `oxc_codegen`, `oxc_minifier`, `oxc_parser`, and `oxc_span`. Its current `unminify_source` performs raw string replacement:

```rust
for (from, to) in [
    ("!0", "true"),
    ("!1", "false"),
    ("void 0", "undefined"),
    ("typeof undefined", "typeof void 0"),
] {
    source = source.replace(from, to);
}
```

This has several correctness problems compared with upstream. It can rewrite comments, strings, property names, and longer identifiers; it does not respect scope; it does not recognize AST shape; and `typeof undefined` is not the upstream `typeof x <|> "u"` transform. It also omits all structural transforms, all transpile transforms, Babel-style parse recovery, and the scope checks for `Infinity`, `JSON`, and `undefined`.

The target's parser currently returns a Python syntax error when any diagnostic is present, whereas upstream requests Babel error recovery. The target also performs `unminify_source` in `transform` regardless of a separate unminify option. These broader pipeline differences should be addressed separately or documented when wiring the new AST pass manager.

## Rust/Oxc equivalents and blockers

### Available Oxc building blocks

Oxc 0.149.0 already provides the core pieces needed for a real port:

* `oxc_ast` contains the JavaScript AST and expression/statement enums.
* `oxc_ast_visit::VisitMut` provides generated mutable traversal, including scope enter/leave hooks.
* `oxc_allocator::AstBuilder` allocates replacement nodes in the parser arena.
* `oxc_semantic::SemanticBuilder` and scoping tables provide symbol/reference analysis.
* `oxc_parser` and `oxc_codegen` parse and print the transformed program.
* `oxc_span` and literal raw fields can preserve source locations and literal spelling where required.

The lockfile already contains transitive `oxc_ast`, `oxc_ast_visit`, and `oxc_semantic`, but direct Rust imports should be made explicit in `Cargo.toml` when implementation begins. The likely direct additions are `oxc_ast`, `oxc_ast_visit`, `oxc_semantic`, and, depending on operator/source-type code, `oxc_syntax`.

### Mapping table

| Upstream concept | Oxc/Rust replacement | Porting concern |
|---|---|---|
| Babel `NodePath` and replace/remove/insert | `VisitMut`, mutable enum fields, arena-allocated replacement nodes, plus explicit parent/context state | There is no general `NodePath` API. Statement-list insertion/removal and parent-sensitive rewrites need helper functions and careful index traversal. |
| `@codemod/matchers` | Typed Rust matcher helpers or direct `match` arms over Oxc enums | Direct matching is more verbose but avoids a second generic matcher framework. Keep shape predicates small and unit-test each one. |
| Babel `scope.hasBinding` | `SemanticBuilder` scoping tables or a lightweight lexical-binding collector | Rebuild semantic information after edits that affect declarations. For local-name checks, a pass-local binding stack may be simpler and safer. |
| Babel `Binding.references` and `constantViolations` | Oxc semantic symbol/reference tables plus write/read classification | Required by temporary-variable and `split-for-loop-vars` transforms. This is the largest infrastructure dependency. Conservative refusal is preferable to an unsafe rewrite. |
| `scope.generateUid('param')` | A deterministic fresh-name helper over the enclosing function/program bindings | Must avoid collisions and reproduce `_param`, `_param2`, etc. closely enough for behavior; exact names are not semantic but tests may snapshot them. |
| Babel `parseExpression` for JSON | Parse a synthesized expression with Oxc, or construct literals recursively from a JSON parser | Parsing synthesized JSON text is easiest but requires an expression parser entry point or a wrapped program parse. A dedicated JSON-to-AST builder makes number/string policy explicit. |
| Babel `path.evaluate()` | Restricted Rust constant evaluator for the exact supported operators | Do not create a general JavaScript evaluator. Implement JavaScript numeric/string coercion only for the matched literal forms, with explicit division and non-finite handling. |
| Babel `extra` raw metadata | Oxc literal raw/value fields and codegen options | Verify Oxc 0.149.0 field names and codegen behavior. Clearing raw spelling must not erase template raw/cooked data accidentally. |
| Babel identifier validator | Oxc identifier-name parser/validator if available, otherwise a small ECMAScript identifier-name helper | Must distinguish identifier names from binding identifiers and preserve reserved/special object/class cases. |
| Babel template builders | `AstBuilder` constructors and direct node assembly | Replacement statements and template elements need allocator-aware construction. |
| `errorRecovery` and `allowReturnOutsideFunction` | Oxc parser options and/or a wrapper policy | Match upstream only if Oxc supports the needed recovery semantics; otherwise retain diagnostics and document the difference. |

### Main blockers and risk areas

1. **Scope data becomes stale after mutation.** Oxc semantic analysis is excellent for a snapshot, but a transform that inserts/removes declarations invalidates reference relationships. Run scope-sensitive passes in groups, rebuild semantic data between groups, or use a local binding collector for the exact query.
2. **Statement-list mutation is nontrivial.** `sequence`, `split-variable-declarations`, `split-for-loop-vars`, and ternary/logic rewrites insert siblings. Implement reusable `Vec<Statement>` rewrite helpers rather than mutating vectors while a generic visitor is iterating them.
3. **JavaScript coercion is not Rust coercion.** `number-expressions`, `infinity`, truncation, and template conversion need explicit ECMAScript semantics. Rust `f64` arithmetic alone is insufficient for string concatenation, signed zero, `NaN`, integer bitwise conversion, and `Infinity` shadowing.
4. **Optional chaining and assignment nodes vary by Oxc version.** Verify exact Oxc enum variants and constructors for optional member/call expressions, assignment operators, assignment patterns, and template elements before implementation.
5. **Source-format snapshots are not identical by default.** Babel and Oxc generators differ in quote selection, whitespace, parentheses, and numeric spelling. Port tests should assert semantic/source patterns first, then establish an Oxc-specific snapshot baseline. Do not treat Babel snapshot byte equality as an automatic requirement.
6. **Error recovery differs.** Upstream intentionally parses recoverable malformed code and continues. The current target rejects any diagnostics. This is a pipeline-level compatibility issue, not a transform-local issue.
7. **The unsafe template transform needs an explicit option.** Upstream aggregate `transpile` includes a transform tagged `unsafe`. The Rust API should either preserve a safe/unsafe policy option or initially omit `template-literals` from the default safe path and expose it behind an explicit flag.

## Recommended Rust implementation plan

1. **Introduce a dedicated AST pass module without changing the public API first.** Add a module such as `src/transforms/unminify.rs` and `src/transforms/transpile.rs`, plus shared `src/transforms/mod.rs`. Keep production behavior unchanged until the pass output is validated.
2. **Add direct Oxc dependencies.** Add `oxc_ast`, `oxc_ast_visit`, `oxc_semantic`, and any required `oxc_syntax`/parser helper dependency at the existing 0.149.0 version. Confirm the public APIs with a small compile-only visitor.
3. **Build pass infrastructure.** Define a `TransformState { changes }`, a pass trait or function convention, parent/context tracking, statement-list utilities, identifier-name validation, fresh-name generation, and a policy flag for safe versus unsafe transforms.
4. **Implement syntax-only preparation first.** Port `block-statements`, `sequence`, `split-variable-declarations`, and `for-to-while`. These establish the statement shapes assumed by later passes and do not require semantic bindings except where sequence target safety is checked structurally.
5. **Implement literal and local expression rewrites.** Port `raw-literals`, `unminify-booleans`, `typeof-undefined`, `unary-expressions`, `merge-strings`, `string-literal-in-template-literal`, `truncate-number-literal`, `number-expressions`, `invert-boolean-logic`, `yoda`, `remove-double-not`, `ternary-to-if`, `logical-to-if`, `merge-else-if`, and `computed-properties`.
6. **Add a restricted constant evaluator.** Support only the exact `number-expressions` matcher set. Define conversion behavior for numeric/string literals, unary minus, arithmetic, exponentiation, remainder, bitwise operators, and integer-only division. Add explicit tests for `NaN`, `Infinity`, negative zero, decimal division, hexadecimal literals, and string numeric coercion.
7. **Add lexical scope infrastructure.** Use `SemanticBuilder` where reliable, supplemented by a pass-local binding stack. Implement queries for local `Infinity`, `JSON`, and `undefined`; do not rewrite when shadowing is uncertain. Add binding/reference/write classification for one-assignment temporary variables and loop test/update usage.
8. **Port scope-sensitive unminify passes.** Implement `infinity`, `json-parse`, `split-for-loop-vars`, and `void-to-undefined`. For JSON, validate the string with a JSON parser and construct an Oxc expression; ensure object keys, arrays, `null`, booleans, strings, and large-number policy are covered.
9. **Port transpile safe transforms.** Implement `default-parameters`, `logical-assignments`, `nullish-coalescing`, `nullish-coalescing-assignment`, and `optional-chaining`. Require exact temporary-variable proof before deleting declarations. Initially implement only the shapes exercised by upstream non-skipped tests.
10. **Port unsafe template literals behind a policy.** Implement `.concat()` and template-plus merging only when the receiver is a literal/template and the option enables unsafe transforms. Preserve escaping rules and document the `ignoreToPrimitiveHint` limitation.
11. **Wire stage boundaries.** Replace textual `unminify_source` in the AST pipeline with: bookmarklet normalization, parse, preparation passes, optional deobfuscation boundary, `transpile` then `unminify`, optional later passes, and code generation. Do not run textual replacement before parsing.
12. **Rebuild semantic analysis between mutation groups.** At minimum, rebuild after preparation, after declaration-splitting/temporary-removal groups, and before any pass that queries references. If rebuilding is too expensive, implement conservative local checks and skip uncertain rewrites.
13. **Create parity tests.** Port every upstream test input to Rust/Python integration tests. Expect generator-format differences; compare normalized output or explicit Oxc snapshots. Retain the six skipped and two todo cases as tracked tests rather than silently expanding behavior.
14. **Add adversarial tests.** Cover comments/strings containing rewrite text, shadowed `Infinity`/`JSON`/`undefined`, invalid computed names, `__proto__`, class constructors, side-effectful computed keys, `typeof undeclared`, loop `let` semantics, duplicate temporary references, decimal division, template coercion, and malformed/recovered input.
15. **Remove or quarantine the old textual pass.** Once the AST path is active, delete the raw replacements or leave them only as a clearly disabled compatibility fallback. Never combine them with the AST path because double rewriting can alter literals and comments.

## Suggested delivery phases and acceptance criteria

| Phase | Deliverable | Acceptance criterion |
|---|---|---|
| A | Oxc visitor/pass scaffolding and preparation transforms | Target compiles; block, sequence, loop, and declaration tests pass without textual replacements. |
| B | Syntax-only unminify transforms | All non-scope unminify cases pass after output normalization; no rewrites occur in comments or strings. |
| C | Constant evaluator and literal metadata | Numeric, JSON-free literal tests pass with explicit non-finite/decimal behavior. |
| D | Scope and temporary-variable analysis | Shadowing and temporary-removal tests pass; uncertain cases are skipped rather than rewritten. |
| E | Safe transpile transforms | Non-skipped default/logical/nullish/optional tests pass. |
| F | Unsafe templates and pipeline integration | Unsafe behavior is opt-in or explicitly documented; `webcrack` stage order is preserved. |
| G | Compatibility hardening | Upstream test inventory is represented, skipped/todo status is tracked, and Python API behavior is tested against current target APIs. |

## Caveats

The upstream tests are focused unit tests, not a proof of semantic equivalence. Several transforms intentionally rely on project heuristics, especially `unary-expressions`, `logical-to-if`, and template-literal conversion. A Rust port should preserve the documented matcher boundaries rather than generalize them opportunistically.

The report analyzes the requested directories and relevant shared engine/configuration files. It does not propose or make edits to production Rust code. The only requested artifact created is this report.

## References

[1]: https://github.com/j4k0xb/webcrack "Webcrack upstream repository"
[2]: https://babeljs.io/docs/babel-parser "Babel parser documentation"
[3]: https://babeljs.io/docs/babel-traverse "Babel traverse documentation"
[4]: https://babeljs.io/docs/babel-types "Babel types documentation"
[5]: https://oxc.rs/docs/learn/architecture "Oxc architecture documentation"
[6]: https://docs.rs/oxc_ast_visit/0.149.0/oxc_ast_visit/ "Oxc AST visitor API documentation"
[7]: https://docs.rs/oxc_semantic/0.149.0/oxc_semantic/ "Oxc semantic analysis API documentation"
[8]: https://docs.rs/oxc_parser/0.149.0/oxc_parser/ "Oxc parser API documentation"

The primary evidence for this report is the checked-out source and tests at `/tmp/upstream-webcrack/packages/webcrack/src/unminify`, `/tmp/upstream-webcrack/packages/webcrack/src/transpile`, the shared engine at `/tmp/upstream-webcrack/packages/webcrack/src/ast-utils/transform.ts`, package metadata at `/tmp/upstream-webcrack/packages/webcrack/package.json`, and the target implementation at `/home/ubuntu/webscrack/src/lib.rs`.
