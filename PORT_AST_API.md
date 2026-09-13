# Webcrack AST/API Port Report: `ast-utils`, `transforms`, `plugin`, and `index`

**Upstream revision:** `c80eec5f00622b86cea871d68349750ce950201f`  
**Upstream root:** `/tmp/upstream-webcrack/packages/webcrack`  
**Target revision:** `305d79f2d679b89e2c5a2f9246f9f12b24f1d8b2`  
**Target root:** `/home/ubuntu/webscrack`  
**Scope:** Every non-test source file in `src/ast-utils`, `src/transforms`, `src/plugin.ts`, and `src/index.ts`, plus the relevant utility, root-API, JSX, mangle, plugin, bookmarklet, and integration tests. No production Rust code was edited.

## Executive conclusion

The requested upstream subsystem is a **Babel AST transformation framework**, not a collection of string rewrites. Its observable contract includes parsing with recovery, mutable AST paths and bindings, scope-aware renaming, structural matchers with captures, visitor composition, JSX reconstruction, optional mangle naming heuristics, staged asynchronous plugins, bookmarklet normalization, progress callbacks, optional unpack/deobfuscation stages, and Babel code-generation conventions. A faithful Rust/Oxc port must therefore replace the current textual compatibility layer with an AST-owned pipeline and must make an explicit policy decision for JavaScript execution.

The target already has a useful Oxc foundation (`oxc_parser`, `oxc_codegen`, `oxc_minifier`, `oxc_allocator`, and `oxc_span`) and Python result/bundle wrappers. It does **not** currently have Oxc semantic analysis, mutable visitor infrastructure, an AST matcher library, a plugin ABI, JSX reconstruction passes, scope-aware mangle/rename support, or an isolated JavaScript VM. The current `unminify_source` function performs raw `String::replace` operations before parsing. That is incompatible with the upstream semantics because it can rewrite comments, strings, property names, and shadowed identifiers. It must be removed or quarantined before AST parity work is accepted.

The least risky plan is incremental. First add Oxc AST visitor and semantic-analysis dependencies and establish arena-safe mutable-pass scaffolding. Then port pure AST utilities and preparation passes, followed by syntax-only transforms. Add a conservative scope layer for rename/mangle. Port legacy and automatic JSX recognition as separate unsafe passes. Finally expose a Rust/Python stage API and either make deobfuscation explicitly unavailable or add a separately audited embedded runtime. Do not claim parity for isolated-vm-backed deobfuscation until that runtime contract is implemented and tested.

## 1. Source inventory and public exports

| Upstream file | Role | Public surface or important dependency |
|---|---|---|
| `src/ast-utils/ast.ts` | Property-name helper | `getPropName` |
| `src/ast-utils/generator.ts` | Babel output and compact preview | `generate`, `codePreview` |
| `src/ast-utils/index.ts` | Barrel | Re-exports `ast`, `generator`, `inline`, `matcher`, `rename`, and `transform` |
| `src/ast-utils/inline.ts` | Inlining and alias removal | `inlineVariable`, `inlineArrayElements`, `inlineObjectProperties`, `inlineFunctionCall`, `inlineFunctionAliases`, `inlineVariableAliases` |
| `src/ast-utils/matcher.ts` | Matchers and binding predicates | `safeLiteral`, `infiniteLoop`, `constKey`, `constObjectProperty`, `anonymousFunction`, `iife`, `constMemberExpression`, literal truthiness/undefined matchers, `findParent`, `findPath`, `createFunctionMatcher`, `varFunctionOrDeclaration`, `isReadonlyObject`, `isTemporaryVariable`, `anySubList` |
| `src/ast-utils/matchers.d.ts` | TypeScript declaration augmentation | More-arity `or`, typed `predicate`, `NodePath.toString()` |
| `src/ast-utils/remove-node-fields.ts` | AST memory/metadata cleanup | `removeNodeFields`; imported directly by root index, not from barrel |
| `src/ast-utils/rename.ts` | Binding rename | `renameFast`, `renameParameters` |
| `src/ast-utils/scope.ts` | UID generation | `generateUid`; imported directly by mangle and JSX transforms |
| `src/ast-utils/transform.ts` | Transform engine types and runners | `applyTransformAsync`, `applyTransform`, `applyTransforms`, `mergeTransforms`, `Transform`, `AsyncTransform`, `TransformState`, `Tag` |
| `src/transforms/jsx.ts` | React classic runtime to JSX | Default unsafe, scope-aware transform named `jsx` |
| `src/transforms/jsx-new.ts` | Automatic JSX runtime to JSX | Default unsafe, scope-aware transform named `jsx-new` |
| `src/transforms/mangle.ts` | Scope-aware identifier renaming | Default safe transform named `mangle`; optional identifier predicate |
| `src/plugin.ts` | User extension API | `Stage`, `PluginState`, `PluginObject`, `PluginAPI`, `Plugin`, `runPlugins` |
| `src/index.ts` | Public `webcrack` pipeline | `webcrack`, `Options`, `WebcrackResult`, and type exports for `Plugin` and `Sandbox` |

There is no `src/plugin/` directory in this checkout. The requested “plugin” subsystem is the single file `src/plugin.ts`. The package root exports only `webcrack`, `Plugin`, and `Sandbox`; the AST utility barrel is an internal import surface and is not re-exported by the package root.

### `ast-utils/ast.ts`

`getPropName(node)` returns a string only for an `Identifier`, `StringLiteral`, or `NumericLiteral`. Identifiers return their names, string literals return their decoded values, and numeric literals return JavaScript `number.toString()` output. All other nodes, including template literals, computed expressions, booleans, null, and private names, return `undefined`. The helper is intentionally narrow and must not be generalized without auditing every caller.

### `ast-utils/generator.ts`

`generate(ast, options = { jsescOption: { minimal: true } })` delegates directly to `@babel/generator` and returns `.code`. `codePreview` generates minified output, suppresses comments, retains the minimal escaping option, and truncates strings longer than 100 characters to the first 70 characters, ` … `, and the final 30 characters. These functions are used for snapshots, diagnostics, and VM setup code. Oxc codegen can replace them for Rust-owned ASTs, but exact whitespace, quote choice, escaping, comments, and truncation will differ unless deliberately normalized.

## 2. AST inline utilities

All inline utilities operate on Babel `NodePath`/`Binding` objects and mutate the existing tree. They are deliberately heuristic. Their contracts include caller preconditions that are not checked at runtime.

### `inlineVariable(binding, value = anyExpression(), unsafeAssignments = false)`

The safe path matches the binding declarator as `identifier(binding.identifier.name)` initialized by `value`, requires `binding.constant`, replaces every reference with the initializer, and removes the declarator. The unsafe path is enabled only when `unsafeAssignments` is true and there is at least one constant violation. It retains only `=` assignments whose left side is the bound identifier and whose right side matches `value`. For each reference it chooses the last matching assignment whose source start is before the reference. It removes assignment expression statements, replaces embedded assignment expressions with their right-hand side, and removes the declaration.

Important boundaries are that assignment ordering is based on source offsets, not control-flow or dominance; assignments inside branches/loops can be mis-modeled; references without an earlier matching assignment are left subject to the later declaration removal; and only exact `=` assignments are recognized. A Rust port should keep the safe path as a conservative data-flow rewrite and gate the unsafe path behind the same explicit option. Do not infer final values across control-flow without a real CFG.

### `inlineArrayElements(array, references)`

For every supplied reference, the function assumes the parent is a member expression and the property is a numeric literal. It reads that numeric value as an array index, clones the corresponding element, and replaces the member expression. It does not validate bounds, holes, spread elements, mutability, writes, aliasing, or computed expressions. The source comment explicitly requires the caller to ensure that the array is immutable and references are valid. Oxc implementation should preserve that caller contract but return/skip safely rather than panic on malformed or out-of-range input.

### `inlineObjectProperties(binding, property = objectProperty())`

The declaration must match `const/let/var name = { ... }` with all properties matching the supplied matcher. A captured array creates a map from `getPropName(property.key)` to the property value. Every reference must be a member access whose property name is in that map; otherwise the whole operation aborts. Matching members are replaced by the original property values and the declaration is removed.

This is not a JavaScript object-semantics proof. It does not reject duplicate keys, getters, `__proto__`, computed side effects that a custom matcher might admit, later mutation, aliasing, or evaluation-order changes. The target port should use cloned nodes and require a semantic proof of read-only object use before removing a declaration. Treat unknown property names and unsupported property forms as a no-op.

### `inlineFunctionCall(fn, caller)`

The normal path assumes the function body’s first statement is a return statement and clones its argument. It traverses the clone without scope analysis and replaces every identifier whose name equals a parameter name with the corresponding caller argument, or `void 0` when an argument is missing. It then replaces the call with the clone. The special case for a rest element in parameter position 1 assumes a function shaped like `function(a, ...b) { return a(...b) }` and directly constructs a call using the caller’s first argument as callee and remaining arguments as arguments.

The utility is designed for small control-flow wrappers. It can be unsound for nested shadowing, duplicate parameter names, default/destructured parameters, `this`, `arguments`, spread evaluation order, or parameter expressions that are not identifiers. Rust should initially support only the tested identifier-parameter/return-expression shapes and skip all other forms.

### `inlineFunctionAliases(binding)`

This utility finds wrapper declarations or function declarations shaped like `function alias(a, b) { return decode(b - 938, a); }` or an equivalent `var alias = function...`. It requires at least two parameters, captures a call to the alias, and requires at least one wrapper parameter reference inside the returned call to avoid treating constant-return functions as wrappers. It recursively follows further aliases, inlines all call references with `inlineFunctionCall`, removes declarations, increments a shared `changes` count, crawls the scope again, and returns `{ changes }`.

The implementation mutates a copied initial reference list but appends further function references while traversing aliases. It assumes a parent scope exists when resolving the alias binding. It should be ported after semantic bindings are available, with recursion-cycle protection and a maximum alias depth. Uncertain wrapper shapes should be skipped.

### `inlineVariableAliases(binding, targetName = binding.identifier.name)`

It finds either `var alias = original` or `alias = original`. For each alias it resolves the alias binding in the reference scope, avoids the trivial `alias = alias` loop, recursively follows further aliases, and either removes the declaration/assignment or replaces the alias reference with `targetName`. It increments `changes` for every mutation. The comment warns that callers must ensure the target name is not shadowed.

A Rust implementation must check scope identity and assignment ordering. Direct textual identifier substitution is not equivalent because nested scopes and object property keys must be distinguished.

## 3. Matcher utilities

The matcher module is built on `@codemod/matchers`. Matchers are structural predicates with captures and key-path tracking. Oxc has no direct equivalent in the target crate, so this is a substantial port surface rather than a thin type translation.

| Export | Exact behavior |
|---|---|
| `safeLiteral` | Matches Babel literals except template literals with expressions. A no-expression template is safe. |
| `infiniteLoop(body?)` | Matches `for(;;)`, `for(; truthy;)`, or `while(truthy)` with optional body matcher. |
| `constKey(name?)` | Identifier or string literal key. |
| `constObjectProperty(value?)` | Non-computed identifier key or non-computed string/numeric key with optional value matcher. |
| `anonymousFunction(params?, body?)` | Anonymous function expression or arrow function with optional params/body matchers. |
| `iife(params?, body?)` | A call whose callee matches `anonymousFunction`. It does not require zero arguments. |
| `constMemberExpression(object, property?)` | Either non-computed `object.property` or computed `object["property"]`; a string object argument is converted to an identifier. Numeric computed properties are not accepted by this helper. |
| `undefinedMatcher` | Identifier `undefined` or `void 0`; it does not account for shadowed `undefined`. |
| `trueMatcher` | `true`, `!0`, `!!1`, or `!![]`. |
| `falseMatcher` | `false` or `![]`. |
| `truthyMatcher` | `trueMatcher` or `[]`; it is intentionally a small syntax matcher, not a JavaScript constant evaluator. |
| `findParent` | Starts at the current path’s parent and returns the first ancestor whose node matches. |
| `findPath` | Starts at the current path and returns the first matching path. |
| `createFunctionMatcher(params, body)` | Captures exactly `params` identifier names and constructs a function-expression matcher whose body callback can refer to those captures. |
| `varFunctionOrDeclaration` | Matches a function declaration or a `var` declaration containing one variable initialized by a function expression. It supports optional id, params, body, generator, and async matchers. |
| `isReadonlyObject(binding, memberAccess)` | Requires every reference to match the supplied member access and rejects assignment/update/delete and destructuring assignment forms. It contains a special workaround for a Babel binding violation that equals the declaration path. |
| `isTemporaryVariable(binding, references, kind)` | Requires exact reference count, exactly one constant violation, and either an uninitialized variable declarator (`var` mode) or an identifier parameter (`param` mode). |
| `AnySubListMatcher` / `anySubList` | Greedily matches requested elements in order while allowing arbitrary elements between them. An empty matcher list matches only an empty array. Captures retain the matched source indexes. |

`matchers.d.ts` augments the third-party package with variadic `or` overloads, a typed `predicate`, and `NodePath.toString()`. These are compile-time conveniences, not runtime exports. A Rust port should not attempt to reproduce the entire third-party matcher API. Implement a small typed pattern layer around Oxc node enums, then port high-value patterns as named functions. Keep `anySubList` and capture state because control-flow and deobfuscation code use ordered subsequence matching.

The `undefinedMatcher` and truthiness matchers are intentionally vulnerable to shadowing and do not evaluate arbitrary expressions. Preserve this heuristic boundary or improve it only with a symbol-aware evaluator and corresponding tests.

## 4. Rename and scope support

### `renameFast(binding, newName)`

`renameFast` walks all binding references. It ignores an export-default declaration reference. Every other reference must be an identifier or JSX identifier; otherwise it throws an error containing the node type and `codePreview`. If the reference scope already has `newName`, it calls Babel scope rename on that existing binding first, effectively moving the conflict aside. It then mutates the reference name.

It separately handles constant violations: direct assignment left identifiers, updates, `delete`, variable declarator identifiers, array-pattern declarators, `for`/assignment-pattern identifiers through a no-scope traversal, and function-declaration identifiers. Unsupported violations throw. Finally it updates the scope binding map by removing the old key, assigning the new key, and changing the binding identifier.

The tests cover conflict renaming (`a` to `b` causes existing `b` to become `_b`), duplicate `var`/function bindings, assignment/update/delete/destructuring/`for in`/`for of`, JSX element names and member namespaces, mixed JSX/plain references, export-default functions, and parameter rename limits. Oxc semantic symbols and reference IDs are required for equivalent behavior. A conservative port should first rename only references proven to resolve to the selected symbol and skip unsupported binding patterns instead of throwing from a production pipeline.

### `renameParameters(path, newNames)`

The helper casts parameters to identifiers, renames the first `min(parameter_count, newNames.length)` bindings, and ignores extra requested names. It therefore does not support destructured/rest parameters faithfully. Port identifier-only parameters first.

### `generateUid(scope, name = "temp")`

The helper produces `toIdentifier(name)` for the first candidate and appends numeric suffixes thereafter. It rejects names already used as labels, bindings, globals, or references. It records the chosen UID in the program scope’s `references` and `uids` tables. Unlike Babel’s regular `generateUid`, it intentionally omits the underscore prefix and name filtering. Oxc needs a scope-local symbol/name allocator with the same collision checks; generate names only after semantic analysis has been refreshed.

## 5. Transform engine

`TransformState` contains only `changes: number`. `Transform` has a name, safe/unsafe tags, optional scope flag, optional synchronous `run(ast, state, options)`, and optional visitor factory. `AsyncTransform` replaces `run` with an async function.

`applyTransformAsync` logs start/end, runs `run`, then traverses the visitor if present. `applyTransform` does the same synchronously and sets `visitor.noScope = !transform.scope`. `applyTransforms` logs a combined name, executes every `run` first, merges all visitor objects with Babel `visitors.merge`, sets `noScope` when requested or when every transform is scope-free, performs one traversal, and returns the accumulated state. `mergeTransforms` creates a transform whose scope flag is true if any child needs scope and whose visitor is the merged child visitor.

The ordering is important. All `run` callbacks happen before the merged visitor. Visitor `exit` handlers can observe replacements made by other visitors. Scope-enabled transforms pay for Babel scope traversal; scope-free transforms intentionally avoid it. A Rust replacement should define an equivalent pass trait with `run`, `visit_mut`, `scope_required`, `tags`, and a shared change counter. Do not merge passes until traversal mutation and replacement ownership are settled. Oxc’s visitor model is suitable for deterministic Rust passes, but it does not provide Babel’s `NodePath` parent/list-key mutation abstraction automatically.

## 6. JSX transforms

Both JSX transforms are tagged `unsafe` and `scope: true`. They reconstruct JSX syntax from call expressions; they do not lower JSX to runtime calls. Oxc’s parser and code generator already understand JSX when the source type is JSX/TSX, so the main work is recognizing call shapes and allocating JSX AST nodes.

### Classic `jsx` transform

It matches `React.createElement(type, props, ...children)` where `type` is an identifier, string literal, or recursively nested non-computed member expression, and `props` is an object expression or `null`. A separate fragment matcher recognizes `React.createElement(React.Fragment, null, ...children)`.

Conversion rules are:

* Identifiers, string literals, and member-expression chains become JSX identifiers/member expressions.
* An object-property identifier or string key becomes a JSX attribute. String values remain a JSX string attribute unless they contain `"` or `\\`, in which case they become an expression container. Non-string values become expression containers.
* Object spread becomes JSX spread attributes.
* String children become JSX text unless they contain `{`, `}`, `<`, `>`, carriage return, or newline; special strings become expression containers.
* Expression children become expression containers. Spread children become JSX spread children. Existing JSX children pass through.
* Fragment calls become `<>...</>` only for the exact null-props fragment matcher. A fragment with a props object, such as `{ key: o }`, remains `<React.Fragment key={o} />`.
* Leading comments on the replaced call are cleared to remove `/*#__PURE__*/` comments.
* A lowercase identifier component is renamed to a generated `Component` UID when a binding exists, avoiding collision with intrinsic HTML tag semantics. If the binding does not exist, the transform returns without converting that call.
* Unsupported object properties throw an error containing a compact code preview. This includes object methods and other property forms.

The tests cover intrinsic/component/member names, conflict renaming, attributes, spreads, nested children, spread children, special text, fragments, fragment props, leading comments, and escaped string attributes. The exact generated quote style is Babel-specific and should be compared after normalization in Rust tests.

### Automatic/new `jsx-new` transform

The transform recognizes default pragma candidates `jsx`, `jsxs`, `_jsx`, `_jsxs`, `jsxDEV`, and `jsxsDEV`. It matches direct identifiers, `(0, r.jsx)` sequence calls, or `object.jsx` member calls. The call shape is `jsx(type, props, key?)`, where props must be an object expression and the optional key is any expression.

It differs from classic JSX in several ways:

* Any type expression is accepted. Identifiers, strings, and deep member expressions become JSX names. Other expressions are hoisted into `const ComponentN = expression` immediately before the containing statement and the generated identifier is used as the JSX tag.
* A lowercase bound identifier is renamed through scope to a generated `_Component`-style UID in the upstream implementation. The legacy transform uses a `Component`-style UID; this difference is observable in snapshots.
* The third argument becomes a `key` JSX attribute.
* `children` and other props are taken from the props object. `jsxs`/`_jsxs` array children are expanded element-by-element; other children are converted as one child.
* A `React.Fragment` type with no resulting attributes becomes a JSX fragment. If a key is present, it remains `<React.Fragment key={...} />` rather than fragment shorthand.
* Object properties named `children` are omitted from attributes. Spread properties become JSX spread attributes. Unsupported properties throw.
* String/child escaping rules match the classic transform.

The tests add arbitrary type hoisting, key handling, array children, automatic fragments, spread children, indirect calls, object-member pragmas, and the same comment/string cases as classic JSX.

Rust/Oxc blocker: Oxc can represent and print JSX, but the target currently lacks `oxc_ast`/`oxc_ast_visit` dependencies and no pass allocates JSX nodes. Add Oxc AST builder support and make source type/JSX parsing explicit. Keep both call recognizers separate because their null/object props and fragment semantics differ.

## 7. Mangle transform

`mangle` is a safe, scope-aware transform with an optional `(id: string) => boolean` predicate. On `BindingIdentifier` exit it ignores import specifiers and object-property keys, applies the predicate, resolves the binding, skips any binding referenced from an export named declaration, infers a readable base name, and calls `renameFast`.

The name inference precedence is:

1. Class declaration name: `C`.
2. Function declaration/name: `f`.
3. Function parameter or assignment-pattern parameter: `p`.
4. `var x = require("module")`: module string, converted to an identifier.
5. Variable declarator: `v` plus a title-cased expression-derived suffix.
6. Catch binding: `e`.
7. Array-pattern binding: `v`.
8. Otherwise retain the existing name.

Expression suffixes are identifiers, named/anonymous functions (`f`), named/anonymous classes (`C`), calls recursively based on the callee, `this`, numeric literals (`LN` plus `toString()`), strings (`LS` plus title-cased first 20 characters), object (`O`), and array (`A`). Other expressions produce no suffix. `titleCase` uppercases letters following the beginning or whitespace and removes characters other than ASCII letters, digits, `$`, and `_`.

Tests require names such as `vLN1`, `vArray`, `vF`, `vC`, `vA`, `vO`, `fs`, `nodeFs`, `vLSHelloWorld`, `C`, `f`, `p`, and `e`; they also test duplicate suffix UIDs, export preservation, predicate filtering, and very long strings. This pass depends on exact scope collision checks and binding mutation. Implement it only after rename and semantic refresh are reliable.

## 8. Plugin API and execution semantics

`Stage` is the union `afterParse | afterPrepare | afterDeobfuscate | afterUnminify | afterUnpack`. `PluginState` is `{ opts: Record<string, unknown> }`; one state object is created per `webcrack` call and shared across all plugin stages. `PluginObject` supports optional `name`, async-or-sync `pre`, async-or-sync `post`, and a Babel `Visitor<PluginState>`. `Plugin` is a factory receiving:

* `parse`: the exact Babel parser function;
* `types`: all Babel type builders/checkers;
* `traverse`: Babel traversal;
* `template`: Babel template builder;
* `matchers`: `@codemod/matchers`.

`runPlugins(ast, plugins, state)` first constructs every plugin object in list order. It then awaits each `pre` sequentially with `this` and the argument both set to the shared state. It merges all visitor objects using Babel `visitors.merge` and traverses once if any visitor exists. It finally awaits each `post` sequentially, again bound to the shared state. Plugin factories are called on every stage invocation, not once globally.

The plugin test creates a plugin with `pre`, `post`, and a `NumericLiteral` visitor that replaces numbers with string literals. Running it in `afterParse` on `1 + 1;` yields `"xx";` and calls both hooks exactly once. The test also includes `runAfter: 'parse'` in the returned object, but `PluginObject` does not declare `runAfter` and `runPlugins` never reads it. It is therefore stale/ignored metadata, not part of the effective contract.

Rust/Python compatibility options are limited. A Rust plugin trait can provide stage name, pre/post callbacks, and a typed AST visitor, but arbitrary Python plugins cannot safely receive Oxc arena references across PyO3 without a designed callback boundary. A pragmatic first API is a Rust-owned plugin registry or Python callbacks receiving generated source/serialized node data, with documented limitations. Do not expose a fake Babel-compatible `types` object. If source compatibility is required, retain a Node/Babel adapter as an optional external mode rather than pretending that Oxc node enums have the same runtime shape.

## 9. Root `index.ts` API and pipeline

### Public types and defaults

`WebcrackResult` contains `code: string`, `bundle: Bundle | undefined`, and `save(path): Promise<void>`. `save` normalizes the path, creates it recursively, writes `deobfuscated.js`, and asks the bundle to save if present.

`Options` contains:

| Option | Default | Behavior |
|---|---:|---|
| `jsx` | `true` | Run classic and automatic JSX reconstruction after unminification/deobfuscation. |
| `unpack` | `true` | Extract bundle modules after code generation. |
| `deobfuscate` | `true` | Run async deobfuscation, including VM-assisted decoding. |
| `unminify` | `true` | Run transpile and unminify passes. |
| `mangle` | `false` | Run mangle with all identifiers, or with a caller predicate. |
| `plugins` | `{}` | Stage-indexed plugin lists. |
| `mappings` | `() => ({})` | Maps unpack matcher keys to codemod matchers. |
| `sandbox` | Browser throwing sandbox or Node isolated-vm sandbox | Executes obfuscator expressions. |
| `onProgress` | no-op | Receives 0 initially and a percentage after every active stage. |

`mergeOptions` mutates the caller’s options object with defaults using `Object.assign`. In Node, the default sandbox is `createNodeSandbox`; in a browser, the default sandbox throws `Custom Sandbox implementation required.`

### Stage order

The active stages are built and falsy entries are removed. The order is:

1. **Parse:** Babel parser with `sourceType: 'unambiguous'`, `allowReturnOutsideFunction: true`, `errorRecovery: true`, and `plugins: ['jsx']`. Parse errors are logged but do not immediately abort when Babel returns a recovered AST.
2. **`afterParse` plugins.**
3. **Prepare:** `removeNodeFields`, then `blockStatements`, `sequence`, and `splitVariableDeclarations` as one transform group named `prepare`.
4. **`afterPrepare` plugins.**
5. **Deobfuscate:** async deobfuscation with the configured sandbox, when enabled.
6. **`afterDeobfuscate` plugins.**
7. **Unminify:** merged `transpile` then `unminify`, when enabled.
8. **`afterUnminify` plugins.**
9. **Mangle**, when enabled, with either an always-true predicate or the supplied predicate.
10. **Late unsafe transforms:** deobfuscation protection cleanup when deobfuscating, and classic/new JSX when `jsx` is enabled. They are intentionally not merged with the main unminify visitor because merged traversal breaks self-defending/debug-protection behavior.
11. **Late deobfuscation cleanup:** `mergeObjectAssignments` and `evaluateGlobals`, when deobfuscating.
12. **Generate:** Babel generator produces the returned `outputCode`.
13. **Unpack:** bundle extraction mutates/reads the same AST after code generation so imports that are moved during extraction do not affect the returned top-level code.
14. **`afterUnpack` plugins.**

Progress starts at `0` before stages and is `(100 / stage_count) * (completed_index + 1)` after every active stage. `afterUnpack` plugins run after `outputCode` is already fixed, so changes they make to the AST do not change `result.code`; they can affect objects consulted by later code only if a future implementation changes this ordering.

### Bookmarklet normalization

Before parsing, a string matching `/^javascript:./` has its `javascript:` prefix removed. It is split at percent signs that are **not** followed by two hexadecimal digits, each piece is passed through `decodeURIComponent`, and pieces are joined with `%`. This decodes valid `%xx` sequences while preserving malformed percent syntax well enough for Babel recovery. It is not equivalent to a blanket `decodeURIComponent` and can throw for some malformed sequences. The upstream test expects a valid encoded bookmarklet to become a formatted IIFE and `%F` to survive as `% F` after parsing/generation.

### Root tests and options

Relevant tests assert that disabling deobfuscation does not crash webpack input, disabling unminify preserves `console["log"](1);`, disabling unpack leaves `bundle` undefined, disabling JSX leaves `React.createElement("div", null);`, a custom sandbox is called once, and mangle removes `foo` from `const foo = 1;`. The combined test expects a `for` declaration split and optional-chaining transform to produce `for (var d = [1], b = 0; ...)` and `null?.length`.

The target’s `Result` already has `code`, optional `Bundle`, diagnostics, and `save`, but its `webcrack` accepts only a Python dictionary for a subset of options and currently routes directly through parsing/codegen/minification. It has no JSX, plugin, progress, mappings, sandbox, parse recovery, or staged AST semantics.

## 10. Babel and isolated-vm dependency map

### Babel dependencies

The package uses the following runtime dependencies:

| Dependency | Use in this subsystem | Oxc/Rust status |
|---|---|---|
| `@babel/parser` | Parse with JSX, unambiguous source type, return-outside-function, and error recovery | `oxc_parser` parses JS/TS/JSX quickly, but target currently treats any diagnostic as fatal and needs explicit recovery policy. |
| `@babel/types` | Node predicates, builders, visitor keys, clone operations, `VISITOR_KEYS`, JSX nodes | Oxc `oxc_ast` enums/builders are the replacement; add allocator-backed builders and adapt all predicates. |
| `@babel/traverse` | `NodePath`, parent/list keys, traversal, scope bindings, references, binding violations, visitor merging | Split between `oxc_ast_visit` and `oxc_semantic`; no direct `NodePath` equivalent. |
| `@babel/generator` | Formatting, compact VM snippets, previews, snapshots | `oxc_codegen` replaces generation, but output formatting is not byte-identical. |
| `@babel/template` | Plugin-provided AST template builder | No direct Oxc equivalent; expose a deliberately smaller Rust template API or keep an external Babel adapter. |
| `@codemod/matchers` | Structural matchers, captures, arbitrary subsequences, predicates | Must be implemented as a Rust-specific pattern module or replaced by typed visitor predicates. |
| `@babel/helper-validator-identifier` | Used elsewhere in the upstream package for computed-property identifier validation; not imported by requested files directly | Oxc identifier validation utilities or a small ECMAScript identifier checker. |
| `debug` | Transform/parse logging | Rust `log`/`tracing` equivalent; target currently has no logging dependency. |

### isolated-vm

`isolated-vm-6` is selected for Node versions below 26 and `isolated-vm-7` for Node 26+. `createNodeSandbox` creates a fresh isolate/context per call, evaluates with a 10-second timeout, copies the result, and uses `file:///obfuscated.js` as filename. It releases the context and disposes the isolate after evaluation. The browser sandbox intentionally throws unless the caller supplies a custom implementation.

`VMDecoder` generates compact string-array, decoder, and optional rotator setup code, evaluates an array of call expressions in the sandbox, and converts certain missing-native-build/version-mismatch errors into an empty result. Other errors are logged with generated VM code and rethrown. This is a security boundary, not merely a convenience dependency.

There is no equivalent in the target Cargo manifest. Adding an arbitrary embedded JavaScript engine is a separate security and licensing decision. `boa_engine`, `deno_core`, `quick_js`, or an external Node helper each have different ES-version, isolation, native-dependency, and Python packaging consequences. The first port should make deobfuscation/sandbox execution explicitly unsupported or require a caller-provided external evaluator. It must not silently execute untrusted input in the host process.

## 11. Rust/Oxc equivalence and blockers

| Upstream capability | Oxc/Rust equivalent | Blocker or caveat |
|---|---|---|
| Babel AST parse/generate | `oxc_parser` + `oxc_codegen` | Add JSX/TS source-type selection, preserve recovered diagnostics, and accept codegen snapshot differences. |
| Babel mutable visitors | `oxc_ast_visit::VisitMut` or custom recursive pass | Arena ownership and replacing nodes require Oxc builders; parent/list-key information is not automatically available. |
| Babel bindings/references | `oxc_semantic` scoping/symbol/reference IDs | Semantic data becomes stale after mutation; rebuild between mutation groups. Confirm exact 0.149 APIs before implementation. |
| Babel scope rename | Symbol-based reference rewriting plus fresh semantic build | Must handle JSX names, assignment patterns, exports, and collisions conservatively. |
| Matcher library | Typed Rust patterns plus helper captures | No direct crate in target; port only used patterns and track source node IDs for captures. |
| Babel cloneNode | Arena allocation or explicit deep clone | Cloning expressions while retaining comments/spans must be intentional. |
| Babel JSX builders | `oxc_ast::ast` JSX variants + `AstBuilder` | Add `oxc_ast`/builder dependencies; ensure codegen emits desired JSX. |
| Babel generator preview | Oxc codegen with a compact/preview helper | Formatting and quote choices differ; use normalized semantic tests. |
| Babel parser recovery | Oxc parse diagnostics/recovered AST where supported | Current target returns `PySyntaxError` for any diagnostic, unlike upstream recovery. |
| Babel templates/plugins | Rust pass API or external Babel compatibility mode | Arbitrary plugin source compatibility is not realistic in a native-only ABI. |
| isolated-vm | External sandbox or embedded JS engine | Security, packaging, timeout, and semantics are unresolved. |
| Async transforms | Rust async trait or synchronous phases plus Python await boundary | Oxc AST arena borrowing and PyO3 GIL interactions complicate async mutation. |
| Bundle/unpack | Existing target placeholder bundle detection | Full upstream unpack is outside this report and must be integrated only after AST output ordering is correct. |

The target should add, at minimum, `oxc_ast`, `oxc_ast_visit`, and `oxc_semantic` at the same pinned Oxc version. `oxc_transformer` may help with standard JSX lowering but does not directly implement this subsystem’s reverse transform from React calls to JSX and should not be treated as a drop-in replacement. A logging crate and test normalization helper are also advisable.

## 12. Concrete implementation plan

### Phase A: establish the Oxc pass substrate

Create an internal Rust AST pipeline around one allocator-owned parsed program. Add a pass trait carrying a name, safe/unsafe tag, scope requirement, and mutable change count. Implement a deterministic `VisitMut` runner and a separate async-capable orchestration layer. Add semantic rebuild checkpoints after preparation and after any binding-changing pass. Preserve parse diagnostics as a result field while distinguishing recoverable diagnostics from fatal parse failure.

Replace textual pre-parse replacements with a bookmarklet normalizer that follows the upstream split/decode/join algorithm. Keep normalization before parsing, but move all `!0`, `!1`, `void 0`, and similar rewrites into AST passes. Add tests proving comments, strings, member keys, and shadowed globals are not rewritten.

### Phase B: port AST utilities and preparation dependencies

Implement `get_prop_name`, code preview, and node metadata cleanup. Port safe literal/member/constant-key predicates and `any_sub_list`. Add conservative expression cloning and array/object property replacement helpers. Port block/sequence/declaration preparation passes needed by the existing root stage before attempting deobfuscation.

Do not port unsafe inline assignment or function-wrapper cases until semantic references and source-order checks exist. For every unsupported shape, skip rather than panic or rewrite.

### Phase C: syntax-only unminify and transform runner parity

Build the Rust transform runner with separate `run` and visitor phases, merged change state, optional scope requirement, and explicit stage names. Port transforms that depend only on node shape before scope. Compare normalized generated output to upstream snapshots rather than exact Babel formatting.

Add test cases from `src/ast-utils/test/inline.test.ts`, `matcher.test.ts`, and the root transform tests. Retain skipped cases for malformed/recovered inputs until Oxc recovery policy is verified.

### Phase D: semantic rename and mangle

Add `oxc_semantic` symbol/reference collection. Implement a symbol-targeted rename operation that updates declarations, references, assignment targets, destructuring targets, `for in/of`, `delete`, function declarations, and JSX element/member namespace names. Implement collision allocation equivalent to `generateUid`, rebuilding semantic state after renames.

Port mangle naming precedence and expression suffix rules exactly. Skip imports, object-property keys, named exports, destructured forms, and unsupported patterns until each has an explicit symbol test. Port all mangle snapshots with output normalization.

### Phase E: JSX reconstruction

Add allocator-backed JSX builders and two separate passes. Start with classic `React.createElement` shapes, then automatic `_jsx`/`jsxs`/`jsxDEV` shapes. Implement nested member names, attributes, spread attributes, children, spread children, string-special-character rules, fragment rules, key handling, comment removal, lowercase component renaming, and arbitrary-type hoisting.

Treat unsupported object properties as no-op in production or as a structured transform diagnostic rather than an unhandled panic. Add every upstream JSX test and adversarial cases for computed/member names, side-effectful props, duplicate children, and shadowed `React`/pragma bindings.

### Phase F: plugin and root API

Define a native plugin interface with stage registration, pre/post hooks, a typed AST callback, and shared opaque state. Expose it to Python only after deciding whether callbacks operate on AST handles or source snapshots. Do not claim Babel `parse`, `types`, `traverse`, `template`, or matcher compatibility unless a Node adapter is retained.

Rebuild `webcrack` stage order exactly: normalize, parse, after-parse plugins, prepare, after-prepare plugins, optional deobfuscation, after-deobfuscation plugins, transpile/unminify, after-unminify plugins, optional mangle, late JSX/deobfuscation cleanup, generate, unpack, and after-unpack plugins. Preserve progress percentages and the upstream default options. Decide whether `afterUnpack` mutations affect returned code; the current upstream behavior says they do not because generation happens earlier.

### Phase G: execution policy and compatibility hardening

Document deobfuscation as unavailable without a sandbox, or implement a separately audited external evaluator with timeout and resource limits. If a Python callable is accepted, invoke it outside AST borrowing scopes and validate returned values before applying mutations. Never execute untrusted JavaScript directly in the Rust host process.

Port root API tests for every option, bookmarklet decoding, parser recovery, progress, custom sandbox behavior, mangle, JSX, plugins, and `save`. Add adversarial tests for comments/strings, shadowed `undefined`/`Infinity`/`JSON`, JSX pragma shadowing, duplicate bindings, `__proto__`, computed property side effects, alias cycles, unsafe assignment control-flow, out-of-range array indexes, and generated-name collisions.

## 13. Acceptance criteria and known caveats

A credible full-port milestone should satisfy all of the following:

1. No production AST transform performs raw source string replacement.
2. All supported rewrites are visitor- and node-type-based, so comments and strings remain untouched.
3. Semantic-dependent passes rebuild symbol/reference information after mutations.
4. Every upstream non-skipped test in the requested areas has a Rust/Python parity test, with formatting normalization where Babel and Oxc differ.
5. JSX, mangle, rename, matcher, plugin, and root-option behavior are each tested independently.
6. Unsupported unsafe forms are skipped and reported rather than guessed.
7. Deobfuscation execution is either implemented with an explicit sandbox contract or clearly reported as unavailable.
8. The public Python result/bundle/save API remains backward compatible with the target’s existing tests.

The upstream tests are focused unit tests, not a proof of semantic equivalence. Several helpers intentionally rely on heuristics, especially truthiness matchers, unsafe assignments, wrapper inlining, JSX reconstruction, and mangle name inference. Exact Babel snapshot formatting is not an appropriate Rust acceptance criterion. The target’s existing placeholder unpacking and textual unminification are outside the requested AST utility source set but directly affect root-index parity and must be replaced or isolated as part of integration.

## References

[1]: https://github.com/j4k0xb/webcrack "Webcrack upstream repository"
[2]: https://babeljs.io/docs/babel-parser "Babel parser documentation"
[3]: https://babeljs.io/docs/babel-traverse "Babel traverse documentation"
[4]: https://babeljs.io/docs/babel-types "Babel types documentation"
[5]: https://oxc.rs/docs/learn/architecture "Oxc architecture documentation"
[6]: https://docs.rs/oxc_ast_visit/0.149.0/oxc_ast_visit/ "Oxc AST visitor API documentation"
[7]: https://docs.rs/oxc_semantic/0.149.0/oxc_semantic/ "Oxc semantic analysis API documentation"
[8]: https://docs.rs/oxc_parser/0.149.0/oxc_parser/ "Oxc parser API documentation"
[9]: https://github.com/laverdet/isolated-vm "isolated-vm project"
[10]: https://docs.rs/pyo3/0.29.0/pyo3/ "PyO3 documentation"

The primary evidence is the checked-out upstream source and tests at the paths stated at the beginning of this report, plus the target implementation at `/home/ubuntu/webscrack/src/lib.rs` and its pinned dependencies in `/home/ubuntu/webscrack/Cargo.toml`.
