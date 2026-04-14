# Extractor Ecosystem

Chisel ships with **stdlib-only** extractors. For production codebases where regex extraction is insufficient, you can plug in **tree-sitter**, **LSP**, or **custom** parsers via `register_extractor()` without adding dependencies to Chisel itself.

## How to use

1. Install the parser **in your environment** (e.g. `pip install tree-sitter`).
2. Copy one of the examples below into your repo.
3. Point `CHISEL_BOOTSTRAP` at your module.
4. Run `chisel analyze`.

```bash
export CHISEL_BOOTSTRAP=myrepo.chisel_extractors
chisel analyze .
```

## Official examples

These examples live in the Chisel repo under `examples/extractors/` and are maintained to match the current `register_extractor` contract.

| Extractor | Language | Backend | Status | File |
|-----------|----------|---------|--------|------|
| JS/TS Tree-sitter | JavaScript, TypeScript | tree-sitter | ✅ Ready | [`examples/extractors/tree_sitter_js_extractor.py`](../examples/extractors/tree_sitter_js_extractor.py) |
| Swift Syntax | Swift | swift-syntax | ✅ Ready | [`examples/extractors/swift_syntax_extractor.py`](../examples/extractors/swift_syntax_extractor.py) |
| LSP Symbols | Python, JS/TS, Rust, C/C++ | LSP | ✅ Ready | [`examples/extractors/lsp_symbol_extractor.py`](../examples/extractors/lsp_symbol_extractor.py) |

### JS/TS Tree-sitter extractor

**Best for:** Closing the dynamic-require gap in JavaScript/TypeScript projects.

**Install:**
```bash
pip install tree-sitter tree-sitter-javascript tree-sitter-typescript
```

**Bootstrap:**
```bash
export CHISEL_BOOTSTRAP=examples.extractors.tree_sitter_js_extractor
```

**What it does:**
- Extracts functions, classes, arrow functions, and methods.
- Tracks variable assignments like `const M = './module'`.
- Resolves `require(M)` to actual module paths (tainted import tracking).
- Falls back to `dynamic_import` for unknown variables.

### Swift Syntax extractor

**Best for:** Swift projects where regex-based extraction misses `@Test` annotations or nested declarations.

**Install:**
```bash
pip install swift-syntax
```

**Bootstrap:**
```bash
export CHISEL_BOOTSTRAP=examples.extractors.swift_syntax_extractor
```

**What it does:**
- Extracts functions, classes, structs, enums, actors, and protocols.
- Uses `swift-syntax` parser for accurate line ranges.

### LSP Symbol extractor

**Best for:** Polyglot repos where you want one generic extractor backed by language servers.

**Install a language server for each language you use:**
```bash
pip install python-lsp-server
npm install -g typescript-language-server
# rust-analyzer and clangd are usually installed via system package manager
```

**Bootstrap:**
```bash
export CHISEL_BOOTSTRAP=examples.extractors.lsp_symbol_extractor
```

**What it does:**
- Spawns the appropriate LSP server per file extension.
- Queries `textDocument/documentSymbol`.
- Maps LSP symbol kinds to Chisel `CodeUnit` types.
- Supports Python, JavaScript, TypeScript, Rust, C, and C++.

## Writing your own

The contract is one function:

```python
from chisel.ast_utils import CodeUnit, register_extractor

def my_extractor(file_path: str, content: str) -> list[CodeUnit]:
    ...

register_extractor("javascript", my_extractor)
```

See [`docs/CUSTOM_EXTRACTORS.md`](CUSTOM_EXTRACTORS.md) for the full contract, bootstrap patterns, and unregister API.

## Maturity guide

| Badge | Meaning |
|-------|---------|
| ✅ Ready | Actively maintained in-repo example, tested against real source files. |
| 🧪 Experimental | Community-provided, may need tweaks for your language server version. |
| 📝 Planned | Listed on the roadmap but no example exists yet. |

## Community extensions

If you write an extractor that others might find useful, open a PR to add it to the `examples/extractors/` directory and this table.

| Extractor | Language | Backend | Author | Status |
|-----------|----------|---------|--------|--------|
| (yours here) | | | | |
