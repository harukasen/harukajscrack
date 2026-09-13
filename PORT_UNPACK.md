# Webcrack `src/unpack` Port Report

**Upstream revision:** `c80eec5f00622b86cea871d68349750ce950201f`  
**Source reviewed:** `/tmp/upstream-webcrack/packages/webcrack/src/unpack`  
**Target reviewed:** `/home/ubuntu/webscrack`  
**Scope:** Every non-test source file under `src/unpack`, all unpack tests and 18 JavaScript fixture/snapshot pairs, plus the shared AST helpers and pipeline/package metadata required to understand behavior. No production Rust code was edited.

## Executive conclusion

The upstream subsystem is an AST-driven **bundle extractor**, not a JavaScript runtime evaluator. It recognizes three webpack shapes and two Browserify wrapper shapes, creates one `Module` object per embedded module, normalizes wrapper parameters to `module`, `exports`, and `require`, rewrites selected webpack runtime idioms into ordinary imports/exports, rewrites numeric module references to relative paths, applies user-provided path mappings, and can save a guarded directory tree with `bundle.json` metadata.

A faithful Rust/Oxc port is practical without Babel or `isolated-vm`. Oxc can parse the same JavaScript, expose typed AST nodes, visit/mutate nodes, generate module code, and perform semantic binding analysis. The main engineering work is replacing Babel matcher composition and Babel scope paths with explicit Oxc predicates plus carefully scoped semantic analysis. The largest compatibility risks are **source-location-preserving mutations**, **binding-aware renaming**, **top-level-only ESM conversion**, and **cross-platform path traversal semantics**. `isolated-vm` is not a direct dependency of `src/unpack`; it is an optional dependency used by the neighboring deobfuscation VM and should not be introduced for unpacking.

The current target is only a compatibility skeleton for unpacking: `detect_bundle` uses source substring heuristics and returns one synthetic module containing the whole source, with an invented entry ID of `0`. It does not extract module containers, resolve dependency paths, perform webpack/Browsify rewrites, apply mappings, or reproduce upstream save behavior. The report therefore recommends implementing unpack as a separate typed AST pipeline rather than extending the current string detector.

## Public and internal exports

The direct exports of the subsystem are small, but the effective API includes methods on the bundle and module classes.

| Upstream export | Shape and behavior | Rust/Oxc port target |
|---|---|---|
| `unpackAST(ast, mappings = {})` from `unpack/index.ts` | Traverses one Babel AST with the webpack 4, webpack 5, webpack chunk, and Browserify visitors. The first matching visitor calls `path.stop()`. It then applies mappings and bundle transforms and returns `Bundle | undefined`. | `unpack_ast(program, mappings)` returning `Option<Bundle>`, with explicit detector precedence and a diagnostics/error policy. |
| `Bundle` from `unpack/bundle.ts` | Base class with `type`, `entryId`, `modules: Map<string, Module>`, `applyMappings`, `save`, and no-op `applyTransforms`. | `Bundle { bundle_type, entry_id, modules: IndexMap<String, Module> }` or `HashMap` plus stable order. Prefer a Rust error result for duplicate mappings, file I/O, and traversal rejection. |
| `BrowserifyBundle` | `Bundle` with type `browserify`. | Thin typed wrapper or enum variant. |
| `WebpackBundle` | `Bundle` with type `webpack`; transforms all modules, then rewrites `require(id)` and import sources to paths. | Thin typed wrapper or enum variant with a webpack-specific transform method. |
| `Module` | `id`, `isEntry`, `path`, Babel `File` AST, and lazy generated `code` cache. Default path is `./index.js` for the entry and `./<id-without-.js>.js` otherwise. | `Module { id, is_entry, path, program, code_cache }`; use an owned allocator per bundle or an arena lifetime strategy. |
| `BrowserifyModule` | Adds `dependencies: Record<number, string>`; the dependency ID is string-keyed in practice despite the TypeScript annotation. | Add `dependencies: IndexMap<String, String>` or `HashMap<String, String>` to the Browserify variant. |
| `WebpackModule` | No fields beyond `Module`; it is a nominal type used by webpack transforms. | Enum/marker variant is sufficient. |
| `Bundle.applyMappings` | Traverses each module without scope analysis. Each mapping matcher may match exactly once globally; a second match throws. A matching path changes the module output path to the mapping path or `node_modules/<mapping>` and stops traversal for that module. Unused mappings do not throw. | Accept typed mapping predicates or a higher-level mapping callback. Preserve one-use and duplicate-use errors. Do not expose Babel matcher objects in Rust. |
| `Bundle.save(path)` | Creates the output directory, writes pretty JSON metadata, then writes every module at its normalized path. Rejects paths whose normalized relative path starts with `..`. | `Bundle::save(&Path)` with `create_dir_all`, JSON serialization, normalized path containment, and atomic/error-aware writes. Preserve the upstream ordering/metadata contract where compatibility matters. |

The upstream package does not export the individual webpack matcher functions from the package root, but `webpack/common-matchers.ts`, `webpack/esm.ts`, `webpack/getDefaultExport.ts`, and `webpack/varInjection.ts` are internal public-to-the-package building blocks. A Rust port should keep them as private modules with unit tests because their boundaries define supported bundle dialects.

## Pipeline and precedence

`unpackAST` creates all four visitors and merges them with Babel's `visitors.merge` in this order:

1. webpack 4 IIFE;
2. webpack 5 block/function form;
3. webpack JSONP/chunk push;
4. Browserify.

The visitor is traversed with scope enabled because webpack 4/5 entry detection relies on bindings. Each matching visitor stops at the matched wrapper, extracts modules, and stores one bundle in a shared `options.bundle`. In normal `webcrack` execution, unpack runs after parsing, JSX/transpile/unminify/deobfuscation preparation, and a code-generation checkpoint. The upstream comment says this checkpoint is needed because unpacking can modify the same AST and can leave imports temporarily outside their normal top-level position.

The Browserify detector is intentionally last. This matters for `browserify-webpack-nested.js`: the top-level Browserify wrapper wins, and the nested webpack text remains part of a Browserify module instead of becoming the outer bundle. The test explicitly asserts `bundle.type === 'browserify'`.

The target's current `webcrack` pipeline still performs `unminify_source` by raw `String::replace` before parsing and then uses `detect_bundle` on source text. That is not semantically equivalent. It can rewrite comments and strings and cannot distinguish nested bundles or identify module boundaries. The port should introduce an AST stage before bundle extraction and keep textual replacement out of the unpack path.

## Data model and path behavior

### Base module paths

`Module` initializes a non-entry module as `./<id-with-.js-suffix-removed>.js`; an entry module is `./index.js`. Webpack IDs are converted through this default unless a mapping changes them. Browserify paths are replaced by dependency-tree resolution when the module is reachable from the selected entry.

The AST is a Babel `File` made from the wrapper function body only:

```text
function (module, exports, require) { statements }
          -> File(Program(statements))
```

The wrapper itself is not retained. Before extraction, the first three parameters are renamed using scope-aware `renameFast` to `require`, `module`, and `exports` in Browserify/webpack order as appropriate. The rename updates references and constant violations and can throw on an unexpected reference shape.

### `relativePath(from, to)`

The helper uses `node:path.posix`:

* A destination beginning with `node_modules/` is returned with that prefix removed. Thus `node_modules/lib/index.js` becomes `lib/index.js` when used as a require source.
* Otherwise it computes `relative(dirname(from), to)`, and prefixes `./` unless the result already starts with `.`.
* It does not force a `.js` suffix and does not normalize missing-module fallback paths beyond the supplied string.

Examples covered by tests are `./a.js -> ./x/y.js` yielding `./x/y.js`, `./x/y.js -> ./a.js` yielding `../a.js`, and `./a.js -> node_modules/lib` yielding `lib`.

### Browserify dependency resolution

`resolveDependencyTree(tree, entry)` recursively follows `tree[moduleId]`, starting with `cwd='.'`. For a relative dependency name (anything beginning with `.`), it joins `cwd` and the name and appends `.js` if the result lacks that suffix. For a package name, it produces `node_modules/<name>/index.js`. It records the dependency path before recursively resolving that dependency, and skips recursion if the dependency ID is already in the accumulated `paths` object. The selected entry is then forcibly set to `./index.js`.

The algorithm computes a prefix from the maximum `path.split('..').length`; it creates `tmp0/tmp1/...` segments to eliminate leading `../` paths, except package paths remain under `node_modules`. This is a containment-oriented synthetic layout, not a filesystem resolution algorithm. It handles cycles by skipping already-seen IDs. It does not correctly infer whether a relative `./utils` refers to `utils.js` or `utils/index.js`; the corresponding test is intentionally skipped and marked FIXME.

The Browserify fixture `browserify-2.js` demonstrates an external dependency `{ vscode: undefined }`: it is retained in the source wrapper but omitted from `dependencies`. Only numeric and string dependency values are followed. The `browserify-cocos2d.js` fixture exercises string module IDs and modules with no dependencies. `browserify.js` covers a four-module graph and a nested relative dependency chain.

### Save and traversal edge cases

`Bundle.save` writes a metadata object containing `type`, `entryId`, and an array of `{ id, path }` records in map iteration order. It creates parent directories and writes generated module code. Before writing each module it normalizes `join(output, module.path)` and rejects it when `relative(output, normalizedPath).startsWith('..')`.

The POSIX test uses module ID `../tmp.js`; the Windows test uses an ID containing backslashes and `..`. Both must be rejected by the corresponding platform's normalization rules. The upstream check is based on Node's native `path`, while `relativePath` and Browserify resolution deliberately use POSIX. A Rust port must make this distinction explicit: use platform-aware `Path`/`normalize` for save containment, but use POSIX-like slash semantics for generated JavaScript module paths. Do not use a simple `trim_start_matches("./")` check as the target currently does; it mishandles platform prefixes and normalized traversal.

## Webpack dialects and exact matching behavior

### Shared module-container matcher

`modulesContainerMatcher` accepts either:

* an array whose elements are anonymous function/arrow-function nodes or `null`; array index is the module ID;
* an object whose properties are numeric, string, or identifier keys with anonymous function values;
* object methods with constant keys;
* a special string-valued `c` property, used for webpack public-path metadata, which is accepted but not extracted as a module.

`getModuleFunctions` emits array indexes for every non-null element. For objects it emits object-method functions and function-valued object properties; the `c` property is ignored because it is not a function. The accepted function type is a non-generator function expression, arrow function with a block body, or object method with the required non-computed/non-generator shape. Arrow functions with expression bodies are not accepted as module functions.

`webpackRequireFunctionMatcher` captures an identifier for the module container and matches a one-parameter function declaration whose body contains, somewhere in an arbitrary sublist, an expression calling either `container[moduleId].call(...)` or `container[moduleId](...)`. It does not require the canonical parameter or statement names. This supports webpack 0.11.x, webpack 4, and webpack 5 invocation forms. The match is syntactic and intentionally narrow.

### Webpack 4 / 0.11.x IIFE

`unpack-webpack-4.ts` matches:

```js
(function (__webpack_modules__) {
  function __webpack_require__(moduleId) {
    // somewhere in the body:
    __webpack_modules__[moduleId].call(...);
  }
  // other statements, including entry invocation
})(modules);
```

The outer function must be a function expression with exactly one captured parameter, and the argument must be a recognized array/object module container. The require function must be a function declaration in the outer body. The invocation may use `.call` or direct invocation. The extractor resolves the require binding from the callee scope and searches its reference paths for either `__webpack_require__.s = <numeric-or-string-id>` or `__webpack_require__(<numeric-or-string-id>)`. The first yields the entry ID; if absent, the first matching require call yields it; otherwise the entry ID is `''` and all modules are non-entry.

Each module is copied from its wrapper body after renaming parameters. A trailing comment whose value is exactly `*` on the final statement is removed to handle development-build `/***/` separators. A `WebpackBundle` is then created.

### Webpack 5 block form

`unpack-webpack-5.ts` searches a `BlockStatement` for an arbitrary-order sublist containing both:

```js
var __webpack_modules__ = { ... }; // or array
function __webpack_require__(moduleId) { ... }
```

The matcher binds the container declaration and require function. It gets the require binding from the matched block, finds only `__webpack_require__.s = <numeric-or-string-id>`, and does not use the fallback `__webpack_require__(id)` search. This is why `webpack-5-no-entry.js` produces a webpack bundle with `entryId: ''` while still extracting modules. It accepts module functions represented as object properties, object methods, or array elements, including webpack 5 method syntax. It marks a module as entry only when its ID equals the discovered assignment.

The implementation has an explicit TODO: it does not support an entry module assignment at the bottom of an IIFE in all webpack 5 layouts. The extractor also does not interpret runtime helper code; it only recognizes the wrapper and copies module bodies.

### Webpack JSONP/chunk push

`unpack-webpack-chunk.ts` matches a call shaped like:

```js
(window.webpackJsonp = window.webpackJsonp || []).push([
  [chunkIds...],
  modules,
  optionalEntryOrRuntime...
]);
```

The global receiver may be an identifier or `this`, and its property must start with `webpack`, covering names such as `self.webpackChunk_N_E`, `window.webpackJsonp`, and `this.webpackJsonp`. The first push-array element must be an array of numeric/string chunk IDs. The second is the module container. Additional elements are accepted but ignored. No entry function is interpreted; every extracted module has `isEntry=false` and the bundle entry ID is `''`. This is an intentional FIXME, not an accidental omission.

### Webpack path rewriting

After runtime transforms, `WebpackBundle.replaceRequirePaths` traverses every module without scope analysis. It matches only `require(<numeric-or-string literal>)` and import declarations with a string source. For each reference it looks up the module ID:

* if found, it replaces the argument/source with `relativePath(current_module.path, dependency.path)`;
* if missing, it uses `./<moduleId>.js` and adds the leading comment `webcrack:missing` to the replacement string literal;
* it does not evaluate computed IDs, variables, concatenated strings, or arbitrary calls.

The same pass handles import declarations produced by ESM conversion. Paths can be odd by design: a mapped path `node_modules/package` is used as a module output path, while `relativePath` strips the `node_modules/` prefix for a require source.

## Webpack runtime transforms

`WebpackBundle.applyTransforms` runs in this exact order:

1. `inlineVarInjections` on each module;
2. `convertESM` on each module;
3. `convertDefaultRequire` across the bundle;
4. `replaceRequirePaths`.

### Inline variable injections

`inlineVarInjections` recognizes a top-level expression statement of the form:

```js
(function (global, other) {
  // body
}.call(thisOrExports, arg1, arg2));
```

The callee must be a non-generator function expression, the call property must be `.call` or `['call']`, the first argument must be `this` or the identifier `exports`, and there must be one or more subsequent arguments. It replaces the entire statement with `var global = arg1; var other = arg2;` followed by the function body statements. It does not replace `this`; the upstream comment relies on `this` already referring to exports. If there are more parameters than arguments, generated `var` declarations use an undefined AST argument path only if the matcher/inputs allow it; the normal supported shape has matching arguments. The transform only scans `program.body`, so nested occurrences are untouched.

The `webpack-var-injection.js` fixture expects two imports after this flattening and subsequent path rewriting.

### ESM conversion

`convertESM` is a top-level-only rewrite. It recognizes:

* `require.r(<identifier>)`: changes `program.sourceType` to `module` and removes the statement;
* `require.d(exports, "name", function () { return value; })`: converts the returned value to an export;
* `require.d(exports, { foo: () => value, bar: () => value })`: converts each property to an export, or appends properties to a prior `const exports = {}` object;
* `const x = require(<numeric literal>)` in an already module-marked module: replaces the declaration with `import * as x from "<numeric-id>"`;
* `module = require.hmd()` at top level: removes the statement.

For a named export whose value is an identifier, it resolves the identifier binding and searches upward for a variable, class, or function declaration. It renames the binding to the export name and wraps the declaration in `export`, producing forms such as `export let counter = 1`, `export class counter {}`, or an exported function. For `default`, a variable declaration becomes `export default <initializer>` and a class/function declaration becomes `export default <declaration>`. If the value is not an identifier, it inserts `export let name = value` after the `require.d` statement, or `export default value` for default.

The implementation uses `findPath` and `renameFast`, so it is binding-aware but assumes supported declaration shapes. It does not generalize computed export names, multiple declarators, complex patterns, assignment-based live bindings, or arbitrary getter bodies. It may produce syntactically valid but semantically simplified exports; parity tests must retain these exact boundaries.

### Default-require compatibility

`convertDefaultRequire` handles webpack's `require.n` helper. It recognizes a variable declaration `const m = require(<numeric literal>)`, followed by either `const getter = require.n(m)`, `require.n(m).a`, or `require.n(m)()`. It resolves the numeric required module through the local binding for `m`. If the required module has `sourceType === 'module'`, it replaces the getter expression with `m.default`; otherwise it uses `m` directly. For a getter variable, it also unwraps references used as a call or member expression, changing `getter.a.prop` or `getter().prop` to `getter.prop` after replacing the initializer.

This helper intentionally does not execute webpack's runtime. It relies on the module's source type after `convertESM`, so transform order is observable.

## Browserify extraction

`browserify/index.ts` recognizes a call with this effective shape:

```js
(function (files, cache, entryIds) { ... })(
  { id: [function (require, module, exports) {}, { depName: depId }] },
  {},
  [entryId, ...]
)
```

It also recognizes the two-stage form where an empty IIFE returns an `init(files, cache, entryIds)` function and that returned function is immediately called. The module table is an object whose keys are numeric literals, string literals, or identifiers. Each value must be `[functionExpression, objectExpression]`; dependency object keys are constant identifiers or strings and values may be numeric literals, string literals, or `undefined`. External `undefined` dependencies are ignored.

Only the first entry ID is captured. Multiple entry points are a TODO. For each table item the function parameters are renamed to `require`, `module`, `exports`, its body becomes a module `File`, and `BrowserifyModule.dependencies` records `depId -> sourceName`. The dependency tree is resolved from the selected entry and paths are applied only to module IDs present in the resolved tree. Unreachable modules retain their default `./<id>.js` path. A non-empty table produces a Browserify bundle.

The top-level visitor stops on the first matching Browserify call. It does not merge multiple bundles and does not traverse into a selected wrapper after extraction.

## Tests and expected coverage

The subsystem has three test files and 18 concurrent sample fixtures, each with a snapshot:

| Test/fixture | Covered behavior |
|---|---|
| `path.test.ts` | POSIX relative paths; two dependency graphs; cycle handling; intentionally skipped `utils/index.js` ambiguity case. |
| `unpack.test.ts` | Browserify wins for nested Browserify+webpack; path mapping; POSIX traversal rejection; Windows traversal rejection. |
| `samples.test.ts` | Every `.js` fixture is unpacked and compared to its `.snap` file. |
| `browserify.js` | Numeric Browserify IDs, nested relative dependencies, wrapper stripping, four-module output. |
| `browserify-2.js` | External `undefined` dependency and string/relative dependency. |
| `browserify-cocos2d.js` | String IDs, sparse/unreachable modules, empty function bodies. |
| `browserify-webpack-nested.js` | Detector precedence: outer Browserify is selected and nested webpack remains module code. |
| `webpack-0.11.x.js` | Older `.call(null, ...)` webpack runtime and array container. |
| `webpack-4.js` | Webpack 4 object container, assigned entry, ESM/default helper paths, missing/normal dependencies. |
| `webpack-5.js` | Webpack 5 block form, object module functions, ESM named/default conversion. |
| `webpack-5-object.js` | Webpack 5 object syntax and non-ESM module. |
| `webpack-5-method.js` | Object method module extraction. |
| `webpack-5-json.js` | JSON-like module body and no entry assignment. |
| `webpack-5-no-entry.js` | Valid extraction with empty `entryId`. |
| `webpack-esm.js` | Named/default ESM runtime conversion, namespace require conversion, CommonJS coexistence. |
| `webpack-var-injection.js` | Top-level `.call(this, ...)` injection flattening. |
| `webpack-object.js` | Classic webpack object container and default require conversion. |
| `webpack-jsonp-chunk.js` | JSONP chunk with string IDs and cross-module require path. |
| `webpack-chunk-no-entries.js` | Chunk with no entry; all modules non-entry and empty entry ID. |
| `webpack-path-traversal.js` | POSIX traversal payload reaches save guard. |
| `webpack-path-traversal-windows.js` | Backslash traversal payload reaches Windows save guard. |

The snapshots also establish formatting expectations. The AST generator may differ between Babel and Oxc, so the Rust test port should compare normalized AST/code where exact whitespace is not contractual, while retaining explicit assertions for module IDs, paths, entry flags, imports/exports, missing comments, and bundle metadata.

## Babel, matcher, and `isolated-vm` dependencies

The unpack source imports these Babel-facing packages:

| Dependency | Use in unpack | Oxc replacement |
|---|---|---|
| `@babel/parser` | Indirectly supplies the AST consumed by `unpackAST` through the main pipeline. | `oxc_parser`. Preserve a JavaScript-compatible source type for bundle fixtures. |
| `@babel/types` | Node type guards, constructors, `File`/`Program`, literals, functions, properties, imports/exports, comments. | `oxc_ast` node enums/structs and allocator-backed constructors. |
| `@babel/traverse` | Visitors, `NodePath`, scope lookup, bindings, reference paths, replacement/removal, `path.stop`, no-scope traversals. | `oxc_ast_visit` for traversal plus `oxc_semantic` for scopes/references; mutation may require direct parent/index walking or a custom mutable visitor. |
| `@babel/template` | Builds `var`, namespace import, named export, and default export nodes. | Construct typed Oxc AST nodes directly; avoid string templates. |
| `@codemod/matchers` | Declarative structural matching with captures, alternatives, arbitrary sublists, and current paths. | Explicit predicate functions returning typed captures and source-node locations. |
| `@babel/generator` | Lazy module code generation through shared `generate`. | `oxc_codegen`. |
| `debug` | One diagnostic log line after bundle extraction. | `tracing`/`log` or omit behind a feature. |

`isolated-vm-6` and `isolated-vm-7` are optional package dependencies in `packages/webcrack/package.json`, but they are used by `src/deobfuscate/vm.ts`, not by any file under `src/unpack`. The unpack port must not embed a JavaScript VM. Runtime evaluation would broaden behavior and create security/performance concerns that upstream intentionally avoids.

## Rust/Oxc equivalence and blockers

### Straightforward equivalents

* Parsing and code generation map directly to `oxc_parser` and `oxc_codegen`, already present in the target manifest.
* AST node classification maps to exhaustive matches over Oxc `Expression`, `Statement`, `Function`, `ObjectPropertyKind`, `ImportDeclaration`, and `ExportDeclaration` variants.
* Module containers can be extracted from mutable AST nodes using typed matching and indexes. Preserve IDs as strings because webpack/Browsify use numeric and arbitrary string IDs.
* Bundle and module metadata should be plain Rust structs with a `BundleKind` enum and `Vec<Module>`/`IndexMap` for deterministic output.
* POSIX dependency path resolution can use `std::path::Component`-free slash logic or a small `path_clean`-style internal helper; it should not use host-native paths for generated module names.
* Save containment can use `std::fs::canonicalize` only with care because output files may not exist yet; lexical normalization plus component-based prefix comparison is preferable, followed by safe directory creation.

### Semantic blockers requiring design decisions

1. **Oxc AST ownership.** A parsed Oxc program borrows/owns nodes in an allocator. A bundle with independent module programs can use one allocator owned by a bundle and copy/extract nodes into new programs, or it can generate module code immediately and discard module ASTs. The latter loses later mapping/transformation flexibility. Choose an owning `BundleArena`/lifetime design before implementing mutation.
2. **Scope-aware parameter renaming.** Babel's `renameFast` updates references and assignments and changes binding tables. Oxc's semantic model is generally analysis-oriented; mutation can invalidate symbols. Implement a conservative local rename pass over the wrapper function with semantic IDs or rebuild semantic analysis after each module extraction. Do not blindly rename matching identifier text because nested shadowed bindings must remain intact.
3. **Parent-aware replacement.** Babel `NodePath.replaceWith`, `remove`, `insertAfter`, and `path.stop` operate with maintained parent paths. Oxc visitors generally need explicit parent/index context. Build a small mutation utility for statement-list replacement/removal and expression replacement, then use it only in known list positions.
4. **Structural matcher captures.** `@codemod/matchers` supports greedy `anySubList`, optional list tails, captures, and matching against parent paths. Recreate only the finite patterns above. Avoid a general matcher engine unless other ports require it; explicit functions will be easier to audit and safer around recovered/error AST nodes.
5. **Source type and generated imports.** `convertESM` mutates `sourceType` from script to module and inserts imports/exports. Oxc codegen must be given the updated source type and valid statement ordering. Preserve the upstream top-level-only behavior rather than hoisting arbitrary imports.
6. **Comments.** The webpack 4 extractor removes a final `/***/` separator represented as a trailing comment whose value is exactly `*`; missing-module replacement adds a leading `webcrack:missing` comment. Oxc comment attachment and codegen APIs must be tested for equivalent output.
7. **Cross-platform paths.** Generated module paths use POSIX semantics; save validation uses platform semantics. The current target's `trim_start_matches("./")` and `Path::join` are insufficient for exact upstream behavior. Add dedicated path tests on Linux and Windows CI or a platform-independent lexical test harness.
8. **Mapping API mismatch.** Upstream accepts Babel matcher objects generated by the public `mappings` callback. The Python target has no equivalent mapping API yet. Decide whether Rust exposes mappings as module ID/path rules, callback-independent literal predicates, or a Python callback bridge. A direct Babel matcher compatibility layer is not realistic.
9. **Error policy.** Upstream throws on duplicate mapping matches and save traversal, while `unpackAST` returns `undefined` when no bundle matches. Rust/PyO3 should return `PyValueError`/`PyIOError` for these failures and `None` for no match. Do not silently fall back to the synthetic whole-source module.
10. **Malformed input.** Upstream receives an AST from the main parser and only matches valid parsed syntax. Oxc recovery nodes and parser diagnostics should be treated as non-matches unless the surrounding API explicitly permits recovered ASTs.

## Concrete implementation plan

1. **Freeze the target API.** Extend the current Rust `Module` and `Bundle` structs only after deciding whether Python users need `code` only or AST-backed later transforms. Preserve `bundle_type`, `entry_id`, `modules`, `is_entry`, `path`, `code`, and `save` names for compatibility.
2. **Add an unpack-owned AST layer.** Parse the normalized source once with Oxc, retain the program in an allocator, and define internal `WebpackModuleAst`/`BrowserifyModuleAst` extraction records. Keep production code changes isolated from the report artifact; this report contains no such edits.
3. **Implement typed path utilities first.** Port `relative_path`, `resolve_dependency_tree`, and lexical save containment. Port all existing path tests, including cycles, package paths, skipped `utils/index.js` ambiguity, POSIX traversal, and Windows backslashes.
4. **Implement module-container inspection.** Add functions for array/object containers, object methods, arrow/function block bodies, null array holes, numeric/string/identifier keys, and ignored `c` metadata. Return module locations and IDs in source order.
5. **Implement wrapper detectors in upstream precedence.** Add webpack 4 IIFE, webpack 5 block form, JSONP/chunk push, then Browserify. For each detector, stop after the first top-level match and preserve the nested-bundle precedence test.
6. **Implement safe wrapper extraction.** Copy module body statements into independent module programs, strip wrappers, detect entry IDs with the exact supported assignments/calls, rename only wrapper parameter bindings, and remove only the exact webpack `/***/` separator comment.
7. **Implement webpack transforms in order.** First flatten top-level `.call(this|exports, ...)` injections. Next convert the exact `require.r`, `require.d`, `require.hmd`, and numeric `require` patterns. Then convert `require.n` forms using extracted module source type. Finally rewrite require/import IDs to relative paths and add `webcrack:missing` on unresolved IDs.
8. **Implement Browserify paths and metadata.** Record dependency edges excluding `undefined`, resolve reachable paths from the first entry, force entry to `./index.js`, leave unreachable modules at defaults, and add Browserify dependency maps to the exposed module model.
9. **Implement mappings as a Rust-native contract.** Initially support a deterministic list of `{match_module_id_or_literal, output_path}` rules or a callback-independent predicate interface. Enforce one global use per mapping and preserve `node_modules/` path interpretation. Document that arbitrary Babel matcher objects are not accepted in the Python port.
10. **Implement bundle save securely.** Serialize valid JSON with escaped strings, normalize each module path lexically, reject any path escaping the destination, create parent directories, and write module code. Add a regression test for both slash styles and a symlink policy if symlinked output directories are allowed.
11. **Add semantic rebuild boundaries.** If Oxc semantic analysis is used for renaming or binding lookup, rebuild it after extraction, after ESM declaration rewrites, and before default-require conversion. If a mutation cannot be proven binding-safe, skip it instead of rewriting text.
12. **Port fixtures and assertions.** Add all 18 fixture files to Rust/Python integration tests. Compare bundle kind, entry ID, IDs, paths, entry flags, dependency maps, and normalized generated code. Keep the skipped Browserify index-directory case and webpack entry/chunk FIXME as explicit tracked limitations.
13. **Replace the current detector only after parity.** Remove or quarantine the substring-based `detect_bundle` and the synthetic module path. Never run it alongside AST extraction, because it can report a false bundle and duplicate output.
14. **Add adversarial tests.** Cover bundle-like text in comments/strings, shadowed `require`/`exports`, computed IDs, expression-bodied arrow modules, duplicate mappings, missing modules, nested bundles, malformed parser recovery, Windows separators, and module IDs containing `..`.

## Recommended acceptance criteria

| Phase | Acceptance criterion |
|---|---|
| A. Models and paths | Bundle/module metadata, POSIX path tests, cycle tests, and save traversal tests pass. |
| B. Detection | All 18 fixtures produce the correct bundle kind, module IDs, entry IDs, and module counts. |
| C. Extraction | Wrapper bodies and parameter names match expected normalized AST/code; Browserify dependency maps are correct. |
| D. Webpack rewrites | ESM, default-require, var-injection, require/import path, missing-module, and comment behaviors match snapshots semantically. |
| E. API | Python `unpack`/`webcrack` returns real extracted modules, `save` writes metadata and files, and no-match returns `None`. |
| F. Hardening | Adversarial tests prove no string/comment rewrites, no traversal, no unsafe scope renames, and stable output ordering. |

## References

The primary evidence is the checked-out upstream source and tests at the paths named above. The external references below document the replacement technologies and upstream concepts.

[1]: https://github.com/j4k0xb/webcrack "Webcrack upstream repository"
[2]: https://babeljs.io/docs/babel-traverse "Babel traverse documentation"
[3]: https://babeljs.io/docs/babel-types "Babel types documentation"
[4]: https://babeljs.io/docs/babel-parser "Babel parser documentation"
[5]: https://oxc.rs/docs/learn/architecture "Oxc architecture documentation"
[6]: https://docs.rs/oxc_ast_visit/0.149.0/oxc_ast_visit/ "Oxc AST visitor API documentation"
[7]: https://docs.rs/oxc_semantic/0.149.0/oxc_semantic/ "Oxc semantic analysis API documentation"
[8]: https://docs.rs/oxc_parser/0.149.0/oxc_parser/ "Oxc parser API documentation"

