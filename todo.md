A lot has been added since this list was written. I updated it by looking at the code; ✅ means done, 🟡 partly done, ⬜ not done. Items whose status changed are in bold.

Code understanding and navigation
- ✅ Go to definition
- ✅ Go to implementation (features/hierarchy.rs)
- ✅ Go to type declaration (definition.rs, type_definition)
- ✅ Type hierarchy and call hierarchy (hierarchy.rs, calls.rs)
- ✅ Find usages and highlight
- ✅ File outline and workspace symbol search
- 🟡 Camel-hump search exists, "search everywhere" does not (that is VS Code's job)
- ✅ Code lens (code_lens.rs): reference counts, implementations, subclasses, overrides

Completion and hints
- ✅ Member, class, function, constant and variable completion
- ✅ Automatic use import
- ✅ Signature help
- ✅ Hover and PHPDoc. Hover now also shows scalar and array types ($data: array).
- ✅ Inlay hints: parameter names and variable types. Classes, arrays, strings and calls with scalar return types are supported (inlay.rs, scalar.rs).
- 🟡 Snippets: only adds parentheses like name($1) when completing functions and methods. No live templates; VS Code's snippet system covers that.
- ✅ Type inference: early return with !instanceof is supported. Conditional return types are now evaluated from the call's arguments (analyze/conditional.rs): config() → Repository, app(User::class) → User.

Inspections and quick fixes
- ✅ Undefined class, function, constant, method and class constant
- ✅ Unused import and unused private member
- ✅ Missing abstract method, deprecated usage
- ✅ Undefined variable (undefvars.rs, undefined-variable)
- ✅ Type mismatch (type_check.rs; arguments and return values checked against native types, aware of strict_types) and wrong argument count (arg_count.rs)
- ✅ Quick fixes: import, optimize imports, generate abstract methods, and a "Change to '…'" suggestion for misspelled names
- ✅ "Add type hint", "add return type", "add PHPDoc" (type_actions.rs)

Refactoring
- ✅ Rename
- ✅ Extract variable (with an option to replace every occurrence) and Inline variable (refactor.rs)
- ✅ Extract method (extract_method.rs): parameters and return values worked out from variable use
- ✅ Move class (move_class.rs, `phpLsp.moveClass` command): new namespace and/or name, PSR-4 file move, updates use lines, names, docblocks and strings
- ⬜ Change signature, pull up/push down

Framework support
- ✅ Eloquent
- ✅ Laravel string support: completion, definition, hover and unknown-key warnings for route(), config(), view(), __()/trans() and env(). Facade and chained calls such as Route::has, Config::get, View::make and
  redirect()->route() are recognized too. Facade aliases (Route::, Str::, package aliases) resolve without a use statement (aliases.rs).
- ✅ Blade (blade.rs, with view variables and their types), Symfony (routes, Twig templates), CodeIgniter 4 (routes, handlers, views, language keys, configs), WordPress
  hooks (completion, definition, hover, callback strings)
- ✅ Composer PSR-4/PSR-0
- ✅ phpstorm-stubs

Formatting and tools
- ✅ Code formatter: built-in formatter (PSR-12, PER, Laravel, Symfony and other presets; PhpStorm code style import/export) and Pint / PHP-CS-Fixer integration. It looks in the project's vendor/bin
  folder first, then on PATH; a custom path can also be set.
- ⬜ Debugger: won't be built. Debugging PHP is done by Xdebug itself; the free PHP Debug extension (`xdebug.php-debug`, maintained by the Xdebug team) is the VS Code front end for it (breakpoints, stepping, variables, conditional breakpoints, Docker path mappings). Possible small additions on our side: a "Set Up Debugging" command that writes `.vscode/launch.json` (with Docker `pathMappings`) and checks that Xdebug and PHP Debug are installed, and LSP inline values (`textDocument/inlineValue`) so variable values show at line ends while debugging.
- ⬜ Test runner, database, HTTP client (not the LSP's job)
- ⬜ Git (VS Code's job)

Performance
- ✅ Parallel indexing
- ✅ Requests are answered while indexing
- ⬜ Persistent index (won't be done, by decision)
