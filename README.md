# php-lsp-rs

A fast PHP language server and indexer written in Rust. Goal: PhpStorm-grade code intelligence for
VS Code (and any other LSP client) on PHP 4 → 8.x code bases — Laravel, Symfony, WordPress, legacy
spaghetti — including very large monorepos.

Companion VS Code extension: [`php-lsp-vsc`](../php-lsp-vsc).

## Status

Early. What works today:

| Feature | Notes |
|---|---|
| Parser | Error-tolerant, PHP 4 → 8.4 syntax (alt syntax, `$s{0}`, `&new`, attributes, enums, match, readonly, first-class callables …). Parses ~1M real vendor files with 0 errors in the sample sets. |
| Workspace indexing | Parallel, streamed, with progress. ~6k files / 31 MB in well under a second. `vendor/` indexed after the project. |
| Built-ins | JetBrains `phpstorm-stubs` embedded, unpacked to `~/.cache/php-lsp/` so *go to definition* opens real files. |
| Go to definition | Classes, functions, constants, methods, properties, class constants, enum cases, variables; follows inheritance, traits, interfaces; composer PSR-4/PSR-0 lazy fallback. |
| Hover | Signature + PHPDoc (`@param`, `@return`, `@throws`, `@deprecated`). |
| Completion | Members (`->`, `::`, `?->`) with type inference, variables, classes (with automatic `use` insertion), functions, constants, keywords. Camel-hump matching (`ProdCon` → `ProductsController`). |
| Signature help | Functions, methods, constructors. |
| Find references / highlight | Name index (per-file set of mentioned names) → candidate files → parallel semantic verification. Classes, functions, constants, methods, properties, class constants, variables. |
| Rename | Prepare + rename for the above (variables, members incl. overrides/implementations, classes incl. `ClassName.php` file rename). Refuses vendor/built-in symbols. |
| Laravel / Eloquent | `Model::where()` & friends forwarded to the query builders (typed `Builder<Model>`), `first/find/create` → model, `get/all` → `Collection<Model>`, relationships as virtual properties (`hasMany` → `Collection<Related>`), `$fillable/$casts/$hidden/$dates/$appends` attributes, `getXAttribute()` accessors, `scopeXxx()` scopes, `foreach` over typed collections. |
| Go to implementation / type hierarchy | Implementations of interfaces, abstract classes and methods (overrides in all descendants); supertypes / subtypes tree (extends, implements, trait use). |
| Docblock class names | Class names in `@var` / `@param` / `@return` / `@throws` / `@property` / `@method` / `@mixin` / `@extends` / `@template … of` / `@see` are references: go to definition, hover, find references and rename work inside comments. |
| Go to type declaration | Class of a variable / `$this`, declared type of a property, method or function (arrays and generics unwrapped). |
| Call hierarchy | Incoming calls (grouped by calling function / method, includes calls through the interface or parent method it overrides, `new Foo` for constructors) and outgoing calls. |
| Inlay hints | Parameter names at call sites (skipped for named / spread args and when the argument is already called the same). |
| Formatting | Built-in formatter with PhpStorm-named options (`phpLsp.format.preset`: `psr12` (default), `per-cs`, `per-cs3`, `laravel`, `symfony`, `pear`, `zend`, `codeigniter`, `phpstorm` — the values of PhpStorm's predefined styles; `phpLsp.format.options` for overrides such as `INDENT_SIZE`, `CONCAT_SPACES`, `SPACE_AFTER_TYPE_CAST`, `SPACE_BEFORE_IF_PARENTHESES`; the settings editor completes all of them). Currently: indentation (blocks, continuation for `->` chains / wrapped expressions, arrays, `switch`, `match`, alternative syntax, comments), spaces (operators, unary / binary `-` `&`, commas, parentheses by kind, brackets, braces, ternaries, return types, named arguments, union / intersection / nullable types, casts), lower-case keywords and `true` / `false` / `null`, line structure (minimum blank lines around namespace / imports / classes / methods, brace placement per kind, one statement per line, blocks opened up, `} else` joined), wrapping long lines (argument / parameter lists and arrays chopped down with `(` / `)` on their own lines, `->` chains where needed, `RIGHT_MARGIN`), braces around single-statement bodies and `else if` → `elseif` (checked: without the added braces the tokens are exactly the original ones), PHPDoc (`@param` / `@var` spacing, tags sorted by weight like PhpStorm, alignment and blank-line options, tabs), trailing commas in multi-line lists (only where the project's PHP version allows them) and `array()` → `[]`, alignment options (`=>`, assignments, class constants, trailing comments, wrapped parameters / arguments / array items), blank lines, trailing spaces, final newline, `// @formatter:off` / `// @formatter:on` regions. Code style files: the project's PhpStorm scheme (`.idea/codeStyles`) replaces the preset, `.editorconfig` files (standard keys and PhpStorm's `ij_php_*` keys) apply on top, `phpLsp.format.options` last (`phpLsp.format.useProjectCodeStyle` turns the files off). *PHP LSP: Import Code Style* reads a PhpStorm scheme / `.editorconfig` into the settings, *Export Code Style* writes the style in effect as either; format settings apply without a restart. *Format Document*, *Format Selection*, indent on Enter / `}`. Only whitespace changes; the result is re-lexed and refused if any token would differ. Strings and heredocs are left as they are; in HTML templates only the PHP is formatted (code lines of multi-line `<?php … ?>` blocks indented from the `<?php` line, spaces between PHP tokens, `<?= $x ?>`), HTML and the tags stay as written; files with `<?` short open tags are not formatted. `phpLsp.formatter.tool`: `auto` uses the project's Pint / PHP-CS-Fixer when it has one (config file or `vendor/bin`), otherwise the built-in formatter. |
| Laravel strings | Completion, go to definition and hover inside `route('…')`, `config('…')`, `view('…')`, `__('…')` / `trans('…')`, `env('…')` and their facade / method forms (`Route::has`, `Config::get`, `View::make`, `Lang::get`, `->route()`, `->view()` …). Routes come from `routes/**` (groups with `as` / `prefix`, `name()` chains, `resource` / `apiResource` with `only` / `except` / `names`, route files loaded through `group(base_path(...))`); config and translation keys from `config/` and `lang/` (nested arrays, JSON translations, all locales); views from `resources/views` (incl. `vendor/` overrides as `pkg::view`); keys from `.env`. |
| Laravel diagnostics | Warnings for route, view, config and translation names that do not exist, with a "did you mean" quick fix. Package routes / config defaults / framework translations from `vendor/` count as defined; existence checks (`Route::has`, `View::exists`, `routeIs`), runtime config writes (`config([...])`, `Config::set`), `pkg::` names, `env()`, plain-sentence translations and dynamic names are never reported. Also scans `app/Providers`, `Modules/*/routes`, `Auth::routes()`, `Route::resources()` and route files `require`d inside groups. Setting: `phpLsp.laravel.diagnostics`. |
| Symbols | Document outline and workspace symbol search. |
| Diagnostics | Syntax errors plus inspections on open files (debounced): undefined class / function / constant / method / class constant, unused imports, unused private members, undefined variables (function bodies only; skips `extract`/`$$x`/`include`), missing abstract method implementations, deprecated usages. Conservative by design: members are only checked when the receiver's whole hierarchy is indexed and has no `__call` / `@mixin`. |
| Quick fixes | Import class (candidates from the index), remove unused import, *Optimize imports* (remove + sort), implement missing abstract methods, create missing method (also in another project file). |
| Type actions | On a function / method / closure header: *Add return type* (inferred from the `return`s, scalars included, or `@return`), *Add type* for parameters (from `@param`, the default value or the parent method) and properties (`@var`, default value, parent property), *Add PHPDoc* (`@param` / `@return` / `@throws`, or `@var`). Only suggested when safe: every returned value must be known, parent signatures and subclass overrides must stay compatible, and the type must exist in the project's PHP version (`phpLsp.phpVersion`, else `composer.json`, else the installed `php`). Classes from other namespaces are imported. |
| Type inference | Native types, `@var` / `@param` / `@return` (`phpstan-`/`psalm-` variants win), `@property` / `@method`, `new`, assignments, method chains, `static`/`$this` returns, `catch` variables, closures. **Arrays:** `User[]`, `array<K,V>`, `list<T>`, literals, `$a[] = …`, `$a[0]`, `foreach`, destructuring, `array_map/filter/merge/pop/…`. **Generics:** `@template` (+ `of` bounds) on functions, methods and classes, `@extends/@implements/@use`, `class-string<T>`, `callable(): T`, conditional return types, `new Box($x)` → `Box<X>`, closure parameter types from collections / builders (`$users->map(fn($u) => …)`). **Narrowing:** `instanceof`, `is_a`, `assert`, guard clauses (`if (!$x instanceof Foo) return;`), `&&` / `\|\|` / ternary, branch merging. Unions and `?->` work in completion and go-to-definition. |

Not yet: refactorings (extract/inline/move), Symfony/Blade/Twig/CI4 specifics, persistent index cache.

## Layout

```
crates/
  php-syntax    lexer + error-tolerant parser + AST + visitor
  php-index     symbol extraction, workspace index, name resolution, type inference
  php-composer  composer.json / installed.json autoload maps
  php-stubs     embedded phpstorm-stubs (Apache-2.0)
  php-lsp       the language server binary (stdio)
```

## Build

```sh
cargo build --release          # → target/release/php-lsp
cargo run -p php-syntax --release --example parse_dir -- /path/to/php/project   # parser smoke test
```

## Design notes

* The server owns the index on a single thread; background workers parse files in parallel and stream
  `FileData` batches over a channel. Requests are served while indexing is still running.
* Open buffers always win over disk; incremental text sync.
* `analyze()` produces a reference list + scopes per file on demand (cheap), so there is no stale
  semantic cache to invalidate.
* Spans are byte offsets; conversion to UTF-16 LSP positions happens at the edge (`LineIndex`).

## License

MIT. Bundled `phpstorm-stubs` are Apache-2.0 (see `crates/php-stubs/LICENSE-phpstorm-stubs`).
