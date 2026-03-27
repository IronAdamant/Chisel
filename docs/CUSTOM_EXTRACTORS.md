# Custom extractors (bring your own parser)

Chisel’s default extractors use the **stdlib only** (`ast` for Python, regex for other languages). If you need **tree-sitter**, an **LSP**, or any **third-party** library, you add it **in your environment** and plug it in via **`register_extractor()`** — not via Chisel’s PyPI dependencies.

## Contract

```python
from chisel.ast_utils import CodeUnit, register_extractor

def my_extractor(file_path: str, content: str) -> list[CodeUnit]:
    ...
```

- **`language`** passed to `register_extractor` must match Chisel’s language id (e.g. `"python"`, `"rust"`) — same as file extension mapping in `ast_utils._EXTENSION_MAP`.
- A registered extractor **replaces** the built-in one for that language for the current process.

## Loading your code before `analyze`

Registration must run **before** Chisel extracts code units. Two supported patterns:

### 1. Environment variable `CHISEL_BOOTSTRAP` (recommended)

Set to a **dotted module path** importable from your `PYTHONPATH`:

```bash
export CHISEL_BOOTSTRAP=myrepo.chisel_plugins
chisel analyze .
```

`chisel_plugins.py` would call `register_extractor(...)`.  
`ChiselEngine` runs `load_user_bootstrap()` at startup (see `chisel/bootstrap.py`).

### 2. Import side effects

Any entrypoint that imports your module **before** creating `ChiselEngine` works (e.g. a thin wrapper script that imports your plugin then calls `chisel.cli:main`).

## Example: tree-sitter (your venv, your deps)

Chisel does **not** install `tree-sitter` or language grammars. In **your** project:

```text
pip install tree-sitter tree-sitter-rust   # example only — versions are yours
```

Then implement `my_extractor` using the tree-sitter API, return `list[CodeUnit]`, and:

```python
register_extractor("rust", my_rust_extractor)
```

## Example stub (stdlib-only)

See **`examples/chisel_bootstrap_example.py`** — documents the hook without adding dependencies; copy and extend in your repo.

## Unregister

```python
from chisel.ast_utils import unregister_extractor
unregister_extractor("python")
```

## Tests

Chisel’s own tests do **not** require third-party parsers. CI stays stdlib-only.
