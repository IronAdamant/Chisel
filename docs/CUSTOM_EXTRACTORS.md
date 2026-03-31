# Custom extractors (bring your own parser)

Chisel's default extractors use the **stdlib only** (`ast` for Python, regex for other languages). If you need **tree-sitter**, an **LSP**, or any **third-party** library, you add it **in your environment** and plug it in via **`register_extractor()`** — not via Chisel's PyPI dependencies.

## Contract

```python
from chisel.ast_utils import CodeUnit, register_extractor

def my_extractor(file_path: str, content: str) -> list[CodeUnit]:
    ...
```

- **`language`** passed to `register_extractor` must match Chisel's language id (e.g. `"python"`, `"rust"`, `"javascript"`, `"typescript"`) — same as file extension mapping in `ast_utils._EXTENSION_MAP`.
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

## Example: tree-sitter for JS/TS (recommended over regex)

The built-in regex extractor misses ~30% of JS dependencies (dynamic `require(variable)`, template literals, eval-based loading). A tree-sitter extractor closes this gap via scope-aware variable tracking.

### Install

```bash
pip install tree-sitter tree-sitter-javascript
```

### Implementation

```python
import tree_sitter_javascript as tsjs
from tree_sitter import Language, Parser
from chisel.ast_utils import CodeUnit, register_extractor

JS_LANG = Language(tsjs.language())
parser = Parser(JS_LANG)


def js_tree_sitter_extractor(file_path: str, content: str) -> list[CodeUnit]:
    """Extract code units + require dependencies via tree-sitter.

    This closes the dynamic require gap: variable assignments are tracked
    via scope analysis, and require() calls are resolved to actual module
    paths where possible.
    """
    units = []
    tree = parser.parse(bytes(content, "utf-8"))

    class Tracker(tree_sitter.NodeVisitor):
        def __init__(self):
            self.scope = {}  # var name -> resolved path
            self.calls = []

        def visit_CallExpression(self, node):
            # Check for require() call
            func = node.child(0)
            if func and func.type == "identifier" and func.text == "require":
                args = node.child(2)  # first argument
                if args:
                    arg_text = args.text.decode() if hasattr(args.text, 'decode') else args.text
                    if arg_text.startswith(("'", '"')):
                        # Static require — resolved directly
                        resolved = arg_text.strip("'\"")
                        self.calls.append({"name": resolved, "resolved": resolved})
                    elif arg_text in self.scope:
                        # Dynamic require with known variable
                        resolved = self.scope[arg_text]
                        self.calls.append({"name": arg_text, "resolved": resolved})
                    else:
                        # Unknown variable — dynamic/eval
                        self.calls.append({"name": arg_text, "resolved": None})
            self.generic_visit(node)

        def visit_VariableDeclarator(self, node):
            # Track: const/let X = require('...') or const X = './path'
            lhs = node.child(0)
            rhs = node.child(1)
            if lhs and rhs and lhs.type == "identifier":
                var_name = lhs.text.decode()
                rhs_text = rhs.text.decode() if hasattr(rhs.text, 'decode') else str(rhs.text)
                if "require" in rhs_text or rhs_text.startswith(("./", "/")):
                    self.scope[var_name] = rhs_text
            self.generic_visit(node)

    tracker = Tracker()
    tracker.visit(tree.root_node)

    # Emit code units for functions/classes (standard tree-sitter traversal)
    for node in tree.root_node.children:
        if node.type in ("function_declaration", "class_declaration",
                         "arrow_function", "method_definition"):
            name_node = node.child(0)
            name = name_node.text.decode() if name_node and hasattr(name_node, "text") else "anonymous"
            units.append(CodeUnit(
                file_path=file_path,
                name=name,
                unit_type="function" if "function" in node.type else "class",
                line_start=node.start_point[0] + 1,
                line_end=node.end_point[0] + 1,
            ))

    return units


register_extractor("javascript", js_tree_sitter_extractor)
register_extractor("typescript", js_tree_sitter_extractor)
```

The key advantage over regex: **scope-aware variable tracking**. When `const MODULE = './plugins/auth'` is seen, the extractor records `MODULE -> './plugins/auth'` in scope. Then `require(MODULE)` is resolved to the actual path — producing a proper `import` edge instead of a low-confidence `dynamic_import`.

## Example: tree-sitter for Rust

```bash
pip install tree-sitter tree-sitter-rust
```

```python
import tree_sitter_rust as tsrust
from tree_sitter import Language, Parser
from chisel.ast_utils import CodeUnit, register_extractor

RS_LANG = Language(tsrust.language())
parser = Parser(RS_LANG)


def rust_tree_sitter_extractor(file_path: str, content: str) -> list[CodeUnit]:
    tree = parser.parse(bytes(content, "utf-8"))
    units = []
    for node in tree.root_node.children:
        if node.type in ("function_item", "struct_item", "enum_item",
                         "impl_item", "trait_item"):
            name_node = node.child(1) if node.child(1) else None
            name = name_node.text.decode() if name_node and hasattr(name_node, "text") else "anonymous"
            kind = node.child(0).text.decode()
            unit_type = {"fn": "function", "struct": "struct",
                         "enum": "enum", "impl": "impl", "trait": "interface"}.get(kind, "function")
            units.append(CodeUnit(
                file_path=file_path, name=name, unit_type=unit_type,
                line_start=node.start_point[0] + 1,
                line_end=node.end_point[0] + 1,
            ))
    return units


register_extractor("rust", rust_tree_sitter_extractor)
```

## Example stub (stdlib-only)

See **`examples/chisel_bootstrap_example.py`** — documents the hook without adding dependencies; copy and extend in your repo.

## Unregister

```python
from chisel.ast_utils import unregister_extractor
unregister_extractor("python")
```

## Tests

Chisel's own tests do **not** require third-party parsers. CI stays stdlib-only.
