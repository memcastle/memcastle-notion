"""Validate a built WASI 0.3 source or embedding component before publishing it."""

import argparse
import os
import re
import subprocess


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("kind", choices=("source", "embedding"))
    parser.add_argument("component")
    args = parser.parse_args()
    wasm_tools = os.environ.get("WASM_TOOLS_BIN", "wasm-tools")
    subprocess.run(
        [
            wasm_tools,
            "validate",
            "--features",
            "cm-async,cm-async-stackful,cm-more-async-builtins",
            args.component,
        ],
        check=True,
    )
    wit = subprocess.check_output(
        [wasm_tools, "component", "wit", args.component], text=True
    )
    # Dependency package definitions are not evidence of what this component actually imports.
    world = wit.split("world root {", 1)[1].split("\n}", 1)[0]
    imports = re.findall(r"^\s*import (wasi:[^;]+);", world, re.MULTILINE)
    if not imports or any(not name.endswith("@0.3.0") for name in imports):
        raise ValueError(
            f"{args.component}: expected WASI 0.3.0 imports, got {imports}"
        )
    export = {
        "source": "memcastle:source/adapter@0.5.0",
        "embedding": "memcastle:embedding/provider@0.2.0",
    }[args.kind]
    if f"export {export};" not in world:
        raise ValueError(f"{args.component}: missing {export} export")
    print(f"Validated {args.component}: {export}, {len(imports)} WASI 0.3.0 imports")


if __name__ == "__main__":
    main()
