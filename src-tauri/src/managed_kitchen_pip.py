"""Protect an installed source-managed Comfy Kitchen from unrelated pip installs.

Invoked with the selected installation Python via `python -c`. All temporary
requirement copies live beside their originals so relative pip paths continue
to resolve as in the source manifests. No repository manifest is modified.
"""
import contextlib
import os
from pathlib import Path
import re
import runpy
import shlex
import sys
import tempfile


def is_kitchen_spec(line):
    value = line.strip()
    if not value or value.startswith("#"):
        return False
    match = re.match(r"^([A-Za-z0-9][A-Za-z0-9._-]*)(?=$|[\s<>=!~;@\[])", value)
    if match and re.sub(r"[-_.]+", "-", match.group(1)).lower() == "comfy-kitchen":
        return True
    if value.startswith(("-e ", "--editable ", "--editable=")):
        return bool(re.search(r"(?:#egg=|/|\\)comfy[-_.]+kitchen(?:[/?#&\s]|$)", value, re.I))
    return False


def _include(line):
    value = line.strip()
    for prefix in ("--requirement=", "--constraint=", "-r", "-c"):
        if value.startswith(prefix):
            kind = "-r" if prefix.startswith(("-r", "--requirement")) else "-c"
            part = value[len(prefix):].strip()
            if not part:
                raise ValueError("empty pip requirements/constraints include")
            return kind, part
    for prefix, kind in (("--requirement", "-r"), ("--constraint", "-c")):
        if value.startswith(prefix + " "):
            return kind, value[len(prefix):].strip()
    return None


def _include_path(value):
    parts = shlex.split(value, comments=True)
    if len(parts) != 1:
        raise ValueError("unsupported pip include path: " + value)
    path = os.path.expandvars(parts[0])
    if "$" in path:
        raise ValueError("unresolved variable in pip include: " + value)
    return path


def rewrite_requirements(source, stack, rewritten=None, visiting=None):
    if rewritten is None:
        rewritten = {}
    if visiting is None:
        visiting = set()
    source = Path(source).resolve(strict=True)
    if source in visiting:
        raise ValueError("cyclic pip requirements include: " + str(source))
    if source in rewritten:
        return rewritten[source]
    visiting.add(source)
    try:
        # Use an adjacent copy, preserving relative -e, -f, wheel, and include paths.
        fd, temp_name = tempfile.mkstemp(prefix=".patcher-kitchen-", suffix=".txt", dir=source.parent)
        os.close(fd)
        target = Path(temp_name)
        stack.callback(target.unlink, missing_ok=True)
        rewritten[source] = target
        lines = source.read_text(encoding="utf-8-sig").splitlines(keepends=True)
        result = []
        index = 0
        while index < len(lines):
            group = [lines[index]]
            index += 1
            while group[-1].rstrip("\r\n").rstrip().endswith("\\"):
                if index == len(lines):
                    raise ValueError("unterminated pip requirements continuation in " + str(source))
                group.append(lines[index])
                index += 1
            first = group[0].strip()
            if is_kitchen_spec(first):
                continue
            include = _include(first) if not first.startswith("#") else None
            if include:
                if len(group) != 1:
                    raise ValueError("multiline pip include is unsupported: " + first)
                kind, ref = include
                child = Path(_include_path(ref))
                if not child.is_absolute():
                    child = source.parent / child
                child_copy = rewrite_requirements(child, stack, rewritten, visiting)
                result.append(kind + " " + shlex.quote(str(child_copy)) + "\n")
            else:
                # Reject unsupported direct Kitchen declarations rather than clobber it.
                if first.startswith(("git+", "hg+", "svn+", "bzr+", "http://", "https://", "-e ", "--editable ")):
                    if "comfy-kitchen" in first.lower().replace("_", "-"):
                        raise ValueError("unrecognized Comfy Kitchen dependency declaration: " + first)
                result.extend(group)
        target.write_text("".join(result), encoding="utf-8")
        return target
    finally:
        visiting.remove(source)


def prepare_install_args(args, version, stack):
    if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9.+!_-]*", version):
        raise ValueError("invalid installed Comfy Kitchen distribution version")
    if not args or args[0] != "install":
        raise ValueError("Kitchen protection requires a pip install step")
    cleaned = ["install"]
    i = 1
    while i < len(args):
        arg = args[i]
        if arg in ("--force-reinstall", "--ignore-installed", "-U", "--upgrade"):
            raise ValueError("a dependency install may not force-reinstall or upgrade managed Kitchen")
        if arg in ("-r", "--requirement", "-c", "--constraint"):
            if i + 1 >= len(args):
                raise ValueError("missing pip requirements/constraints path")
            kind = "-r" if arg in ("-r", "--requirement") else "-c"
            cleaned.extend((kind, str(rewrite_requirements(args[i + 1], stack))))
            i += 2
        elif arg.startswith(("--requirement=", "--constraint=")):
            kind = "-r" if arg.startswith("--requirement=") else "-c"
            cleaned.extend((kind, str(rewrite_requirements(arg.split("=", 1)[1], stack))))
            i += 1
        elif is_kitchen_spec(arg):
            i += 1
        else:
            cleaned.append(arg)
            i += 1
    # A pyproject can declare only comfy-kitchen. Filtering that sole direct
    # dependency leaves no work for pip and must be a successful no-op.
    if cleaned == ["install"]:
        return []
    fd, constraint_name = tempfile.mkstemp(prefix="patcher-kitchen-", suffix=".txt")
    os.close(fd)
    constraint = Path(constraint_name)
    stack.callback(constraint.unlink, missing_ok=True)
    constraint.write_text("comfy-kitchen==" + version + "\n", encoding="utf-8")
    cleaned.extend(("-c", str(constraint)))
    return cleaned


def main():
    if len(sys.argv) < 3:
        raise ValueError("expected installed Kitchen version and pip install arguments")
    with contextlib.ExitStack() as stack:
        args = prepare_install_args(sys.argv[2:], sys.argv[1], stack)
        if not args:
            return
        sys.argv = ["pip", *args]
        runpy.run_module("pip", run_name="__main__", alter_sys=True)


if __name__ == "__main__":
    main()
