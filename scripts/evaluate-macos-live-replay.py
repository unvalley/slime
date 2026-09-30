#!/usr/bin/env python3
"""Replay input through an isolated copy of a model-bundled macOS QA app."""

import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("app", type=Path)
    parser.add_argument("inputs", type=Path)
    parser.add_argument("output", type=Path, help="new directory; never overwrites a run")
    parser.add_argument("--direct-live-enter", action="store_true",
                        help="commit the LIVE display directly, without Space conversion")
    parser.add_argument("--final-live-only", action="store_true",
                        help="rank once after all input, representing a burst before the debounce fires")
    args = parser.parse_args()
    repo = Path(__file__).resolve().parent.parent
    app, inputs, output = (p.resolve() for p in (args.app, args.inputs, args.output))
    library = app / "Contents/Frameworks/libslime_ffi.dylib"
    models = list((app / "Contents/Resources").rglob("*.gguf"))
    if not library.is_file() or not models:
        parser.error("app must contain the FFI library and a GGUF model")
    rows = json.loads(inputs.read_text())
    if not isinstance(rows, list) or not rows:
        parser.error("inputs must be a non-empty JSON array")
    ids = set()
    for row in rows:
        key = str(row["index"])
        if key in ids:
            parser.error(f"duplicate index: {key}")
        ids.add(key)
        if not all(isinstance(row.get(k), str) for k in ("input", "context_text")):
            parser.error(f"invalid input/context at {key}")
        expected = row.get("expected_output")
        if not isinstance(expected, list) or not expected or not all(
            isinstance(value, str) for value in expected
        ):
            parser.error(f"invalid expected outputs at {key}")
    output.mkdir(parents=True, exist_ok=False)
    bundle = output / "Replay.app"
    shutil.copytree(app, bundle)
    frozen_inputs = output / "inputs.json"
    shutil.copyfile(inputs, frozen_inputs)
    executable = bundle / "Contents/MacOS/Slime"
    sources = [repo / "platforms/macos/Sources/RustEngine.swift",
               repo / "platforms/macos/Sources/UserDataStore.swift",
               repo / "platforms/macos/Tests/LiveReplayProbe.swift"]
    header = repo / "crates/slime-ffi/include/slime_ffi.h"
    manifest = {"app": str(app), "items": len(rows), "sha256": {
        str(path): hashlib.sha256(path.read_bytes()).hexdigest()
        for path in [frozen_inputs, library, header, *models, *sources]
    }, "mode": "isolated history-disabled profiles"}
    manifest["ranking_schedule"] = "after-input" if args.final_live_only else "after-each-scalar"
    manifest["commit_mode"] = "direct-live-enter" if args.direct_live_enter else "space-enter"
    (output / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    with (output / "build.log").open("w") as log:
        def run(argv):
            subprocess.run(argv, check=True, stdout=log, stderr=log)
        run(["swiftc", "-swift-version", "5", "-module-name", "Slime",
             "-import-objc-header", str(header), "-framework", "AppKit",
             "-L", str(bundle / "Contents/Frameworks"), "-lslime_ffi",
             "-Xlinker", "-rpath", "-Xlinker", "@executable_path/../Frameworks",
             *map(str, sources), "-o", str(executable)])
        linked = subprocess.check_output(["otool", "-L", str(executable)], text=True)
        dependencies = [line.strip().split(" (", 1)[0] for line in linked.splitlines()[1:]]
        ffi = [name for name in dependencies if name.endswith("/libslime_ffi.dylib")]
        if len(ffi) != 1:
            raise RuntimeError("expected exactly one linked FFI library")
        run(["install_name_tool", "-change", ffi[0], "@rpath/libslime_ffi.dylib", str(executable)])
        run(["codesign", "--force", "--deep", "--sign", "-", str(bundle)])
        run(["codesign", "--verify", "--deep", "--strict", str(bundle)])
    pending = output / "results.pending.json"
    with pending.open("w") as out, (output / "run.log").open("w") as log:
        subprocess.run([str(executable), str(output / "profiles"), str(frozen_inputs),
                        *(["--direct-live-enter"] if args.direct_live_enter else []),
                        *(["--final-live-only"] if args.final_live_only else [])],
                       stdout=out, stderr=log, check=True)
    results = json.loads(pending.read_text())
    if len(results) != len(rows) or {str(row["index"]) for row in results} != ids:
        raise RuntimeError("incomplete replay output")
    pending.rename(output / "results.json")
    print(output / "results.json")


if __name__ == "__main__":
    main()
