# Port Analysis: `packages/webcrack/src/deobfuscate`

## Executive conclusion

The upstream `deobfuscate` subsystem is a pattern-driven, multi-stage rewrite pipeline for JavaScript obfuscation families, especially javascript-obfuscator output. Its core job is not general JavaScript evaluation. It recognizes a small set of structural templates, proves or approximates read-only use with Babel bindings, evaluates decoder calls in an isolated JavaScript VM, and then removes the scaffolding that made the obfuscation work.

A Rust/Oxc port is feasible, but it is not a direct visitor translation. Oxc provides the parser, arena-backed AST, semantic analysis, traversal, and code generation needed for the port. The substantial missing layer is Babel's mutable `NodePath` plus `@codemod/matchers` pattern/capture system. The other major design decision is the decoder runtime: upstream executes attacker-controlled decoder setup code in `isolated-vm`; the current Rust target has no embedded JavaScript runtime and no sandbox callback in its public API.

The recommended port is staged. First implement conservative, static AST transforms and a custom binding/reference index. Then add a restricted decoder evaluator or an explicitly configured embedded runtime. Do not silently replace isolated execution with ordinary host-process evaluation. Preserve upstream's safe/unsafe distinction, no-op behavior for uncertain shapes, decode-error comments, and fixture snapshots.

This report analyzes upstream revision `c80eec5f00622b86cea871d68349750ce950201f` and target revision `305d79f2d679b89e2c5a2f9246f9f12b24f1d8b2`.

## 1. Audit scope and file inventory

The audited source directory contains fifteen non-test TypeScript files and 1,539 source lines. The test directory contains eight test files, a 28-line inline snapshot file, fifteen JavaScript sample inputs, and fifteen expected sample snapshots.

| Source file | Role | Export surface |
|---|---|---|
| `index.ts` | Async orchestration for string-array decoder deobfuscation | Default async transform; re-exports sandbox constructors and `Sandbox` type |
| `array-rotator.ts` | Detects the decoder-array rotation IIFE | `ArrayRotator` type; `findArrayRotator` |
| `control-flow-object.ts` | Resolves object-dispatched control flow | Default transform |
| `control-flow-switch.ts` | Linearizes sequence-array `while/switch` control flow | Default transform |
| `dead-code.ts` | Selects branches for literal string comparisons | Default transform |
| `debug-protection.ts` | Removes javascript-obfuscator debug-protection templates | Default transform |
| `decoder.ts` | Models decoder functions and collects decodable calls | `Decoder` class; `findDecoders` |
| `evaluate-globals.ts` | Replaces unshadowed `atob` and URI calls | Default transform |
| `inline-decoded-strings.ts` | Replaces decoder calls with VM results | Default async transform |
| `inline-decoder-wrappers.ts` | Removes aliases around decoder functions | Default transform |
| `inline-object-props.ts` | Inlines literal object property reads | Default transform |
| `merge-object-assignments.ts` | Folds contiguous `obj.key = value` statements into an object literal | Default transform |
| `self-defending.ts` | Removes single-call controller/self-defending templates | Default transform |
| `string-array.ts` | Finds obfuscator string arrays and simple immutable arrays | `StringArray` type; `findStringArray` |
| `vm.ts` | Sandbox abstraction and decoder execution | `Sandbox`, `createNodeSandbox`, `createBrowserSandbox`, `VMDecoder` |

The relevant tests are `control-flow-object.test.ts`, `control-flow-switch.test.ts`, `dead-code.test.ts`, `deobfuscate.test.ts`, `evaluate-globals.test.ts`, `inline-object-props.test.ts`, `merge-object-assignments.test.ts`, and `samples.test.ts`. The `deobfuscate.test.ts` file tests the shared AST alias inliners rather than the top-level deobfuscate transform directly. The sample suite exercises the integrated pipeline against fifteen obfuscator fixtures.

## 2. Public exports and pipeline boundaries

### 2.1 Public exports

`src/deobfuscate/index.ts` exports the following runtime API:

| Export | Behavior |
|---|---|
| Default transform | An async `Transform<Sandbox>` named `deobfuscate`, tagged `unsafe`, with scope analysis enabled |
| `createNodeSandbox()` | Returns a `Sandbox` that creates a fresh `isolated-vm` isolate/context for each call |
| `createBrowserSandbox()` | Returns a function that always throws because no browser-safe implementation is supplied |
| `Sandbox` | Type alias `(code: string) => Promise<unknown>` |

The package root also re-exports `Sandbox` and `createNodeSandbox`. The internal classes and detector functions are not exposed by the package root, although they are exported from their individual module files.

### 2.2 Conditions for doing work

The default transform immediately returns when no sandbox is supplied. It also returns when no recognized string-array function/array is found. Therefore `deobfuscate` is intentionally inert without an execution policy and does not attempt a best-effort static decoder rewrite.

When a string array is found, the transform performs these operations in order:

1. Detect a rotation IIFE associated with the string-array references.
2. Find decoder functions that call the normalized string-array function and index its result.
3. Inline literal object arguments used by decoder calls.
4. Inline variable and function aliases around each decoder.
5. Collect decoder calls, execute the string array/decoder/rotator setup in the sandbox, and replace calls with returned values.
6. Remove the string-array declaration, rotator expression, and decoder declarations if at least one decoder was found.
7. Apply string merging, dead-code selection, object-dispatch control-flow removal, and sequence-switch linearization.

The root `src/index.ts` adds related transforms outside `deobfuscate/index.ts`. After the main deobfuscation and unminification stages, it applies `self-defending` and `debug-protection`; it then applies `merge-object-assignments` and `evaluate-globals`. This distinction matters because those files are in the audited directory but are not invoked by the default transform's own `run` method.

### 2.3 Root pipeline ordering

The package root parses with Babel using `sourceType: 'unambiguous'`, `allowReturnOutsideFunction`, error recovery, and JSX support. It prepares the tree, runs the async deobfuscation transform before unminification, runs unminification, then runs self-defending/debug-protection, then merge-object-assignments/evaluate-globals, and finally generates code. This ordering is deliberate: some template detectors need pre-unminified shapes, while self-defending/debug protection are intentionally kept out of the merged unminify visitor.

The current target Rust crate has Oxc parsing/code generation/minification only. Its Python `deobfuscate()` currently delegates to `unminify()` and does not implement this subsystem. No production Rust code was changed for this report.

## 3. Detailed behavior by module

### 3.1 `string-array.ts`: string-array discovery

`findStringArray(ast)` recognizes two wrapped forms emitted by later javascript-obfuscator versions:

* a function declaration or variable function whose body declares an array of string literals or `undefined` holes and returns an immediately assigned function that returns the array; and
* a form that assigns the function expression, then returns a call to the named function.

The matcher captures the function name, array identifier, and array expression. On a match it renames the binding to `__STRING_ARRAY__`, records the path, all binding reference paths, original name, normalized name, and array length, then stops traversal.

It also recognizes a simple standalone variable declaration such as `const arr = ['log', 'Hello'];`. It removes and inlines the array only when the binding is referenced, all recognized member accesses are numeric indices below the array length, and `isReadonlyObject` proves the array is not mutated or aliased in a disallowed way. Mutable arrays are left intact. The integrated `simple-string-array.js` fixture demonstrates that the immutable array is inlined while a later assignment to `arr2[0]` prevents the mutable array from being changed; an unreferenced `arr3` is also left in place.

Edge cases include sparse arrays because `undefined` is an accepted element matcher, numeric-index bounds, and strict dependence on Babel's binding/reference analysis. The detector is template-specific; it does not evaluate arbitrary arrays, computed indices, spread elements, or aliases without the expected shape.

### 3.2 `array-rotator.ts`: rotation detection

`findArrayRotator(stringArray)` searches references to the normalized string-array function for an expression statement containing an IIFE, or its unary-negated form, with the following broad structure: declarations, then an infinite loop containing a `try` and `catch`, where both branches contain `array.push(array.shift())` and the try branch contains `parseInt` somewhere in the matched loop.

The returned `ArrayRotator` is the expression-statement path. It is not executed as a separate AST transform. The VM decoder serializes the rotator into setup code so that decoder indices observe the same rotated array as the obfuscated program. After decoding, the root transform removes the rotator path along with the array and decoder declarations.

The matcher intentionally accepts both `iife(...)` and `!iife(...)`, which covers the unary rotator fixture. It does not prove exact runtime equivalence beyond the structural template. If no rotator is found, VM setup contains no rotation code.

### 3.3 `decoder.ts`: decoder discovery and call collection

A `Decoder` stores the original function name, normalized name such as `__DECODE_0__`, and the Babel path for its declaration or variable declaration. `findDecoders` walks references to the string-array function and finds enclosing functions containing both `var array = __STRING_ARRAY__()` and a member access such as `array[index]` or `array[index -= offset]`. Each matched binding is renamed and recorded.

`Decoder.collectCalls()` identifies calls with all-literal arguments. A literal argument may be a number, string, unary negative literal, or recursively binary expression whose operands are themselves accepted literal arguments. It also handles a call whose single argument is a conditional expression by replacing `decode(test ? a : b)` with `test ? decode(a) : decode(b)`, then crawling scope information again. Calls with nonliteral expression arguments receive one attempt at inlining referenced variables; they are collected only if they become literal calls.

A bare decoder identifier used as an expression statement, such as `decode;`, is removed. Other nonliteral calls remain uncollected. The call collector mutates the AST while iterating binding references, so a Rust port must snapshot references before edits and refresh semantic data after structural rewrites.

The implementation assumes decoder paths and bindings exist after matching. A production Rust version should treat missing bindings, unsupported parameters, and stale references as safe no-ops rather than panic conditions.

### 3.4 `inline-object-props.ts`: literal decoder-argument objects

This safe transform handles objects containing only string or numeric literal properties. For a direct expression such as `({ x: 1 }).x`, it replaces the member expression with `1`. For a declaration such as `const obj = { x: 1 }; console.log(decode(obj.x));`, it requires a binding and a read-only-object proof, then replaces eligible member reads and removes the declaration through `inlineObjectProperties`.

Property names are restricted to word characters and are resolved through `constKey`/`getPropName`, which supports constant identifier, string, and numeric keys according to the shared AST utility rules. Missing properties, shared references, variable reassignment, property writes, destructuring writes, `delete`, and update expressions are deliberately ignored. The tests require all of those cases to remain unchanged.

This transform runs before decoder call collection because obfuscators commonly store numeric/string decoder arguments in a temporary object.

### 3.5 `inline-decoder-wrappers.ts`: alias removal

For each decoder, the transform resolves its binding and invokes the shared `inlineVariableAliases` and `inlineFunctionAliases` helpers. Variable aliases such as `const alias = decoder` become direct decoder references. Function wrappers such as `function alias(a, b) { return decoder(a - 625, b); }` are inlined into calls and recursively followed.

The shared function inliner is intentionally narrow. It targets return-expression wrappers with identifier parameters and substitutes arguments into the cloned return expression. It is not a general JavaScript beta-reducer. Nested shadowing, `this`, `arguments`, defaults, destructuring, complex spreads, and arbitrary control flow must not be inferred as equivalent.

The `deobfuscate.test.ts` snapshots verify that direct decoder calls remain, variable aliases collapse at every nested scope, and function aliases preserve argument transformations while removing the alias function. The test also demonstrates the need to distinguish a function declaration's binding from a nested alias binding.

### 3.6 `vm.ts`: execution model

`Sandbox` is an asynchronous function accepting generated JavaScript source and returning an unknown value. `createNodeSandbox()` dynamically imports `isolated-vm-7` on Node major version 26 or later and `isolated-vm-6` on earlier supported Node versions. It creates a fresh isolate and context for each evaluation, uses a 10-second timeout, copies the result out of the isolate, labels the source `file:///obfuscated.js`, releases the context, and disposes the isolate.

`createBrowserSandbox()` is only a placeholder. It always throws `Custom Sandbox implementation required.` and explicitly notes that the intended browser library is unavailable in web workers.

`VMDecoder` generates compact, comment-free code for the string-array declaration, all decoder declarations, and the optional rotator. Compact output is required because self-defending obfuscators inspect function source text. It evaluates an IIFE returning an array containing the collected call expressions. Missing optional native module errors and known isolated-vm ABI/native-build errors are logged and converted to an empty result. Other evaluation errors are rethrown after logging the generated VM code through the debug logger.

The VM is a semantic and security boundary. Decoder code can execute arbitrary JavaScript available in the isolate. The upstream code relies on isolated-vm rather than Node's ordinary `eval`; the port must not substitute host-process evaluation without an explicit security decision.

### 3.7 `inline-decoded-strings.ts`: VM result substitution

This async transform collects calls from every decoder. If there are none, it does nothing. Otherwise it asks `VMDecoder.decode` for values, replaces each call for which a result exists with Babel's `valueToNode`, and adds a leading `webcrack:decode_error` comment when the returned value is not a string. Thus `undefined`, numbers, booleans, and failed/partial values can remain in the output but are visibly marked.

The change count is incremented by the number of collected calls, not by the number of successful string results. If the sandbox returns fewer values than calls, the unmatched calls remain in the tree. A Rust port should preserve this partial-result behavior and should use explicit AST constructors for supported JavaScript values rather than treating every VM value as a string.

### 3.8 `control-flow-object.ts`: object-dispatched control flow

This safe transform recognizes an object whose keys are exactly five alphabetic characters, case-insensitive, and whose values are one of the following:

* a numeric-sequence string such as `"6|0|4|3|1|5|2"`;
* a two-parameter function returning one of several operand-order variants of a binary or logical expression;
* an arbitrary-arity function returning a call of its first parameter with the remaining parameters; or
* a two-parameter rest wrapper returning `a(...b)`.

It resolves member reads from the object or an alias, replacing string values directly and inlining recognized function calls. It requires a constant binding and a read-only-object proof. It supports forked obfuscator output in which some properties are assigned after an initially empty object declaration: it merges contiguous assignments, applies the `mergeStrings` transform to assignment statements, validates an alias and reference count, adopts the alias references, and removes the alias declaration.

Unknown properties are preserved and receive a leading `webcrack:control_flow_missing_prop` comment. The transform loops over old references in reverse to avoid a known Babel replacement/reference invalidation issue, then recursively processes declarations that contain references introduced by replacement. Direct object literals are handled separately by a `MemberExpression` visitor.

The test suite covers direct literal property access returning a string, direct function-member access returning the function expression, and function-call inlining producing `u === undefined`. Sample fixtures cover complete and partial key assignment, split strings, spread wrappers, and switch/return combinations. Unsupported object values, mutable bindings, nonconstant keys, and aliasing must remain unchanged.

### 3.9 `control-flow-switch.ts`: sequence switch linearization

This safe transform recognizes a block containing:

1. a variable initialized from a numeric sequence string's `.split('|')`;
2. an iterator declaration with any initializer shape accepted by the matcher; and
3. an infinite loop containing `switch(sequence[iterator++]) { ... }` followed by `break`.

Case tests must be numeric strings. The transform builds a map from case label to consequent statements, removes a trailing `continue` from each case, then replaces the first three block statements with case bodies in sequence order. It counts removed setup statements plus inserted statements as changes.

The matcher is structural and does not perform a CFG proof. The source uses non-null assertions when looking up sequence labels, so malformed but matcher-compatible inputs can be unsafe. A Rust port should validate every sequence label and preserve unmatched blocks rather than indexing an absent case. The test for a `return` inside a case verifies that returns survive flattening and that the enclosing function remains syntactically valid.

### 3.10 `dead-code.ts`: literal string branch selection

This transform is tagged unsafe and has scope analysis enabled. It only matches `===`, `==`, `!==`, or `!=` between two string literals, optionally wrapped in unary `!`. It asks Babel `evaluateTruthy()` for the test value and replaces an `if`/conditional expression with the selected branch, removes a false branch with no alternate, or removes the whole conditional.

When replacing an `if` with a block body, it manually merges child bindings into the parent scope. If a child binding collides with a parent binding, it generates a UID and renames the child binding before splicing statements into the parent. This is required because the resulting statements no longer have the original block boundary.

The test suite verifies true branch, false branch, removal without an alternate, and scope collision resulting in `let _foo = 2`. Rust must not flatten blocks without equivalent declaration and lexical-scope handling.

### 3.11 `merge-object-assignments.ts`: contiguous assignment folding

This safe transform recognizes a statement-list declaration such as `const obj = {};` followed immediately by `obj.foo = value;` statements. It appends each property to the object expression and removes the assignment statements. Computed string and numeric properties are normalized to ordinary object keys; other computed expressions remain computed.

It stops when the next sibling is not an exact assignment statement or when a circular-reference guard fires. The guard rejects direct references such as `obj.foo = obj` and all call expressions such as `obj.foo = fn()` because the called function might capture or mutate the object. This conservative behavior is intentional.

If the object has exactly one remaining reference, consists only of safe literals, arrays, and recursively safe objects, and the reference is not in a repeatable context, the declaration is removed and the object expression is substituted at the reference. Repeatable contexts include loops, functions, object methods, and class bodies because inlining would change object allocation count. The test suite covers computed keys, circular references, calls, and every repeatable context.

The transform crawls program scope before edits, manually dereferences the removed assignments, and shifts reference paths. These are Babel-specific mutable-binding operations that need a custom replacement/index update in Rust.

### 3.12 `evaluate-globals.ts`: safe global string evaluation

This safe transform replaces calls of the exact shape `atob("...")`, `unescape("...")`, `decodeURI("...")`, and `decodeURIComponent("...")` when the name is not locally bound. It invokes the JavaScript global function with `globalThis` as receiver, converts the result to a string literal, and increments the change count. Exceptions, including malformed Base64 or URI input, are swallowed and leave the original call unchanged.

The receiver detail matters for browser APIs that throw `TypeError: Illegal invocation` when detached. A Rust port should implement JavaScript-compatible behavior rather than using host-language approximations. `atob` needs browser Base64 rules; `unescape` and URI decoders need percent-decoding and error behavior. Shadowed local functions must never be rewritten.

### 3.13 `self-defending.ts`: single-call controller removal

This safe transform recognizes a javascript-obfuscator single-call controller initialized from an IIFE. The IIFE declares a `firstCall` flag and returns a function that, on the first call, optionally invokes `fn.apply(context, arguments)`, nulls `fn`, returns the result, and thereafter returns an empty function. It removes controller call sites, including the nested call shape used by debug-protection calls, removes associated wrapper declarations, removes empty leftover IIFEs, and removes calls to generated self-defending functions.

The pattern is deliberately tied to a specific template. It also documents compatibility with self-defending, domain-lock, console-output, and debug-protection-function-call templates from javascript-obfuscator. A Rust implementation should use a narrowly validated template matcher and should not delete arbitrary IIFEs or `apply` calls.

### 3.14 `debug-protection.ts`: debug interval removal

This safe transform matches debug-protection function declarations or variable functions with a nested debugger-protection function. The nested function contains either a `debugger` statement or a dynamically constructed `Function('debugger')` call, then recursively calls itself with an incremented counter. The outer function conditionally returns the nested function or invokes it inside a try block.

After matching, it finds references to the outer function, removes any containing IIFE that calls `setInterval(outer, numericDelay)`, and removes the matched declaration. It does not attempt to evaluate or preserve the protection logic. The detector is based on upstream javascript-obfuscator template URLs and should be ported with fixture-driven structural tests.

## 4. Tests and expected coverage

The test suite has two layers. Unit tests invoke individual transforms through a Babel parse/generate harness. Integrated sample tests call the package-level `webcrack(code)` API with default options and compare generated code to file snapshots.

| Test group | Covered behavior |
|---|---|
| `deobfuscate.test.ts` | Variable alias and function-wrapper alias inlining, nested scopes, argument arithmetic |
| `evaluate-globals.test.ts` | Successful `atob`, `unescape`, URI decoding, and malformed `atob` preservation |
| `control-flow-object.test.ts` | Direct object literal string/function access and function call inlining |
| `control-flow-switch.test.ts` | Sequence switch flattening with an early return |
| `dead-code.test.ts` | True/false string comparisons, removal without else, scope collision |
| `inline-object-props.test.ts` | Literal object access and direct literal access; all major mutation/aliasing no-op guards |
| `merge-object-assignments.test.ts` | Assignment folding, computed keys, cycles, calls, functions, methods, classes, and loops |
| `samples.test.ts` | Fifteen integrated obfuscator/simple-array fixtures and exact output snapshots |

The fifteen integrated samples cover calls transform, complete and partial control-flow keys, split strings, spread wrappers, switch returns, generic control flow, external variables, function wrappers, high obfuscation, multiple encoders, unary rotator, undefined decoder results, variable-function forms, the main v4 fixture, and a simple string array. Expected snapshots demonstrate that successful decoder values become strings, non-string results become `undefined` with `/*webcrack:decode_error*/`, control flow is linearized, and mutable or unreferenced arrays are not incorrectly removed.

The upstream test command was not executed in this checkout because `/tmp/upstream-webcrack` has no `node_modules` directory. The source and fixture inventory was read directly. A Rust port should import all fifteen inputs and snapshots as regression fixtures rather than relying only on the small unit-test set.

## 5. Babel, matcher, and isolated-vm dependency map

| Upstream dependency | Use in this subsystem | Rust/Oxc direction |
|---|---|---|
| `@babel/types` | Node guards, literal/function/property constructors, AST mutation | `oxc_ast` nodes plus allocator-backed constructors |
| `@babel/traverse` | `NodePath`, visitors, parent paths, bindings, references, scope crawling, `evaluateTruthy` | `oxc_ast_visit` for traversal plus `oxc_semantic` and custom mutation/context indexes |
| `@babel/parser` | Parsing in root pipeline and unit harness | Existing `oxc_parser` |
| `@babel/generator` | Compact VM setup serialization and final output | Existing `oxc_codegen`; verify compact output options and function serialization |
| `@babel/template` | Builds extracted conditional expression in `Decoder.collectCalls` | Construct `ConditionalExpression` and cloned calls directly |
| `@codemod/matchers` | Structural templates, captures, `anySubList`, recursive predicates | No direct Oxc equivalent; implement named typed matcher functions or a small internal pattern DSL |
| `debug` | Diagnostics and VM error logging | Rust `tracing`/`log` or existing target diagnostics convention |
| `isolated-vm-6` / `isolated-vm-7` | Fresh, timeout-bound decoder execution | No current target equivalent; choose static evaluator, embedded JS engine, or explicit external sandbox |

The target `Cargo.toml` already contains `oxc_allocator`, `oxc_codegen`, `oxc_minifier`, `oxc_parser`, and `oxc_span`, all at version `0.149.0`. It does not directly list `oxc_ast_visit`, `oxc_semantic`, or an embedded JavaScript engine. The lockfile contains Oxc semantic/traverse-related packages transitively, but they should be added as direct dependencies only after checking the exact public APIs used by the port.

`@codemod/matchers` is more than syntactic convenience. Captures are reused across nested predicates, `anySubList` finds ordered noncontiguous statements, and matchers carry enough path context to support the rewrite. Replacing it with ad hoc string searches would cause false positives. The Rust design should have one typed detector per template and shared helpers for constant keys, member accesses, literal expressions, and statement-list patterns.

## 6. Rust/Oxc equivalents and blockers

### 6.1 Straightforward mappings

Parsing and final code generation map to `oxc_parser` and `oxc_codegen`, already used by the target. Arena allocation maps to `oxc_allocator`. AST node enums and constructors map to `oxc_ast`. Numeric/string/boolean/null/undefined-like values can be represented with Oxc expression variants, with care around JavaScript's identifier `undefined` versus an actual absent value.

Traversal maps to `oxc_ast_visit` for read-only discovery. Semantic analysis maps to `oxc_semantic` for lexical bindings, references, writes, and scope identity. Existing Oxc traversal crates should be used directly rather than creating a Babel-like object model.

### 6.2 Major blockers

**Mutable `NodePath` semantics.** Babel provides parent paths, sibling paths, scope objects, replacement, removal, comment attachment, and path-local traversal. Oxc visitors do not provide a drop-in replacement. Mutating an arena AST during traversal can invalidate assumptions about indexes and references. The port needs an explicit rewrite layer that either collects edits first or traverses mutable statement lists with stable indices and re-runs semantic analysis after each structural phase.

**Binding/reference model.** `isReadonlyObject`, alias inlining, decoder discovery, dead-code scope merging, and object assignment folding all depend on Babel's binding/reference paths. Oxc semantic IDs are a good basis, but edits change declarations and references. Use semantic analysis per pass or maintain a pass-local binding index with declaration IDs, reference IDs, writes, and enclosing scope IDs. Do not use text-based identifier replacement.

**Pattern matching.** There is no direct Oxc equivalent of `@codemod/matchers`. A matcher DSL could be built, but it risks reproducing a large amount of Babel-specific machinery. Named Rust detector functions are safer for the first port. Each detector should validate the entire template and return a typed match record containing node IDs, statement indexes, names, and cloned values.

**Scope-preserving block flattening.** `dead-code` merges a selected block into its parent. Rust must rename colliding lexical bindings and update all references before moving statements. It is safer initially to preserve a block when binding collisions or nontrivial declarations exist, then add a proven scope-rewrite helper.

**Function-wrapper inlining.** The upstream helper is intentionally unsound outside small templates. Rust should initially support only identifier parameters, a single return expression, and known expression/call shapes. Unsupported defaults, destructuring, nested scopes, `this`, `arguments`, and spread evaluation should produce no change.

**JavaScript execution.** `isolated-vm` is a Node native module and cannot be linked as a Rust crate. A static evaluator can cover simple array indexing, arithmetic, Base64/RC4-like decoder code only if its semantics are implemented explicitly, but it will not cover arbitrary obfuscator-generated JavaScript. An embedded engine such as Boa or QuickJS/rquickjs introduces new dependencies and its own sandbox/resource policy. A subprocess Node evaluator is easier to prototype but is a weaker isolation/performance choice and must be strictly timeout- and IPC-controlled.

**JavaScript-compatible global codecs.** `atob`, `unescape`, `decodeURI`, and `decodeURIComponent` have browser/ECMAScript edge behavior that differs from common Rust Base64 and URL libraries. Implement and test the exact accepted alphabet, padding, malformed-input errors, UTF-8 handling, and reserved URI characters before enabling these rewrites.

**Generated setup source.** Upstream deliberately emits compact function source to evade self-defending `toString` checks. Oxc codegen must support compact output with comments disabled for the setup fragment. If compact codegen output differs in meaningful whitespace or parentheses, decoder evaluation may fail even though the AST is correct.

**Change counts and comments.** The `changes` count is part of transform state and is used by the wider pipeline. Preserve count semantics where externally observable. Preserve exact diagnostic comments `webcrack:decode_error` and `webcrack:control_flow_missing_prop` or define a documented Rust-compatible spelling before fixture migration.

## 7. Recommended architecture for the Rust port

Use a `deobfuscate` module with explicit phases rather than one monolithic visitor. A practical internal representation is:

```text
DeobfuscateContext {
    allocator,
    semantic_index,
    diagnostics,
    changes,
    execution_policy,
}
```

A semantic index should provide declaration identity, scope identity, reference lists, write/reference classification, constant-binding checks, and repeatable-context queries. Each phase should return edits or mutate one well-defined list, then invalidate/rebuild the index before the next phase that relies on bindings.

Represent detected structures with typed records such as `StringArrayInfo`, `DecoderInfo`, `RotatorInfo`, `ObjectDispatchInfo`, and `SequenceSwitchInfo`. Store stable AST/node IDs or source spans rather than raw mutable references. Clone expression subtrees when moving values to avoid aliasing arena nodes.

Keep safe and unsafe policies explicit. Safe transforms should only run when the semantic proof succeeds. Unsafe transforms should be opt-in or clearly marked in API state, matching upstream tags. The VM/execution path must have a separate resource policy for timeout, memory, result size, and allowed globals.

## 8. Concrete implementation plan

1. **Add the AST/semantic substrate.** Add direct `oxc_ast`, `oxc_ast_visit`, and `oxc_semantic` dependencies at `0.149.0`. Compile a small visitor that records lexical scopes, declarations, references, writes, parent statement lists, and source spans. Do not change production behavior yet.

2. **Build safe mutation helpers.** Implement constructors and edit helpers for replacing expressions, deleting statements, inserting statement lists, cloning subtrees, adding leading comments, and normalizing constant property keys. Require all edits to be applied through these helpers.

3. **Implement read-only proofs.** Port the necessary subset of `isReadonlyObject`, constant-binding checks, alias detection, circular-reference checks, and repeatable-context checks. Unknown writes, aliases, computed side effects, and unsupported patterns must return no-op.

4. **Port literal-only transforms first.** Implement `inline-object-props`, `merge-object-assignments`, `evaluate-globals`, `dead-code`, and `control-flow-switch`. Add unit tests corresponding exactly to upstream snapshots before attempting decoder execution.

5. **Port control-flow objects.** Implement `control-flow-object` with typed pattern detectors for string dispatch, binary/logical wrappers, arbitrary-arity call wrappers, rest wrappers, aliases, and assigned-later properties. Add recursion depth and malformed-case guards.

6. **Port self-defending/debug protection.** Add narrowly scoped template detectors for `self-defending` and `debug-protection`. Run these in the same root-stage position as upstream, after unminification. Preserve uncertain functions rather than deleting them.

7. **Port string-array and decoder discovery.** Implement `find_string_array`, rotator detection, decoder discovery, literal call collection, conditional call extraction, and wrapper alias inlining. Rebuild semantic indexes after alias edits and before collecting calls.

8. **Choose and isolate the execution strategy.** First provide an execution-policy interface. Implement a restricted static decoder evaluator if it can cover the fixture corpus. In parallel, prototype an embedded engine or external sandbox behind the interface. Never make ordinary host `eval` the default. Return an empty decode result on optional-engine unavailability, matching upstream's graceful native-module behavior.

9. **Implement VM result substitution.** Serialize compact setup code, execute all calls in one request, preserve partial result behavior, construct supported returned values, and attach `webcrack:decode_error` to non-string values. Enforce timeout and result-size limits.

10. **Integrate root ordering.** Run string-array/decoder phases before unminification, then self-defending/debug protection and merge/evaluate phases in their upstream positions. Keep `deobfuscate` disabled when no execution policy is configured, unless a documented static-only mode is explicitly selected.

11. **Migrate fixtures and add differential tests.** Copy all fifteen upstream sample inputs and expected snapshots into Rust tests. Add focused tests for every upstream unit test, malformed/missing switch cases, shadowed globals, alias shadowing, sparse arrays, comments, Unicode, URI errors, and engine failures.

12. **Document compatibility levels.** Publish a capability matrix distinguishing exact parity, conservative no-op, and unavailable execution-dependent behavior. Include engine choice, security limitations, and whether Python callers can supply a sandbox callback.

## 9. Acceptance criteria and caveats

A first safe-static milestone should pass all literal-object, assignment-merge, global-evaluation, dead-code, and switch unit tests without changing unsupported input. It should preserve comments and output snapshots modulo an explicitly approved Oxc formatting difference.

A decoder milestone should pass all simple-string-array and obfuscator fixtures that do not require unsupported runtime behavior. It should reproduce string values, partial results, `undefined` decode-error comments, wrapper removal, rotator alignment, conditional-call extraction, and cleanup ordering.

Full parity requires an execution engine capable of running arbitrary obfuscator decoder setup under timeout/resource limits. Without that engine, the port cannot honestly claim parity with the upstream `isolated-vm` path. The current target public API also lacks an option corresponding to upstream `sandbox`, so the API design must be extended or the feature must remain opt-in and static-only.

The upstream implementation itself is template- and version-sensitive. It can intentionally leave output unchanged, add diagnostic comments, or return no decoded values when a native module is absent. These are compatibility behaviors, not bugs to erase. A Rust port should preserve conservative no-op behavior and make any deliberate deviations visible in diagnostics.

## References

[1]: https://github.com/j4k0xb/webcrack/tree/c80eec5f00622b86cea871d68349750ce950201f "Upstream webcrack source revision audited for this report"

[2]: https://docs.rs/oxc_ast_visit/0.149.0/oxc_ast_visit/ "Oxc AST visitor API documentation"

[3]: https://docs.rs/oxc_semantic/0.149.0/oxc_semantic/ "Oxc semantic analysis API documentation"

[4]: https://docs.rs/oxc_parser/0.149.0/oxc_parser/ "Oxc parser API documentation"

[5]: https://docs.rs/oxc_codegen/0.149.0/oxc_codegen/ "Oxc code generation API documentation"

[6]: https://github.com/nicolo-ribaudo/isolated-vm "isolated-vm project and native isolate model"

[7]: https://github.com/javascript-obfuscator/javascript-obfuscator "javascript-obfuscator templates and output families referenced by upstream detectors"

[8]: https://github.com/j4k0xb/webcrack/issues/98 "Upstream control-flow-object regression referenced by its test"

[9]: https://github.com/j4k0xb/webcrack/issues/162 "Upstream control-flow-switch regression referenced by its test"

[10]: https://vitest.dev/guide/snapshot.html "Vitest snapshot testing documentation"

All behavioral claims above are grounded primarily in the audited upstream source and tests [1]. Oxc API recommendations refer to the target's pinned crate family [2] [3] [4] [5].

---

**Author:** Manus AI
**Artifact status:** Analysis only. No production Rust code was edited.
