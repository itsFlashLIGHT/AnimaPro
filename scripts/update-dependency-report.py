#!/usr/bin/env python3
"""Generate a conservative list from Cargo metadata; requires Python 3 and Cargo."""
import argparse
import hashlib
import json
import pathlib
import subprocess
import sys

root = pathlib.Path(__file__).resolve().parent.parent
parser = argparse.ArgumentParser()
parser.add_argument("--check", action="store_true", help="fail if the checked-in report differs")
parser.add_argument("--offline", action="store_true", help="use only locally cached crates")
args = parser.parse_args()
command = ["cargo", "metadata", "--locked", "--format-version", "1"]
if args.offline:
    command.append("--offline")
metadata = json.loads(subprocess.check_output(command, cwd=root))
digest = hashlib.sha256((root / "Cargo.lock").read_bytes()).hexdigest()
packages = sorted((p for p in metadata["packages"] if p["source"]), key=lambda p: (p["name"].lower(), p["version"]))
lines = [
    "# Rust dependencies from Cargo.lock",
    f"# Cargo.lock SHA-256: {digest}",
    "# Includes all resolved platforms and build dependencies; license strings come from package manifests.",
    "# Name\tVersion\tDeclared license\tRepository",
]
for package in packages:
    def clean(value):
        return (value or "NOT DECLARED").replace("\t", " ").replace("\n", " ")
    lines.append("\t".join(map(clean, [package["name"], package["version"], package.get("license"), package.get("repository") or package.get("homepage")])))
outputs = {
    root / "docs" / "THIRD_PARTY_DEPENDENCIES.tsv": "\n".join(lines) + "\n",
    root / "docs" / "dependencies-lock.sha256": digest + "\n",
}
for path, content in outputs.items():
    if args.check:
        if not path.exists() or path.read_text(encoding="utf-8") != content:
            print(f"Outdated dependency report: {path}", file=sys.stderr)
            sys.exit(1)
    else:
        path.write_text(content, encoding="utf-8")
print(f"{len(packages)} packages; Cargo.lock SHA-256 {digest}")
