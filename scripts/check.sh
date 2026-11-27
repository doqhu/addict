#!/usr/bin/env bash
# Check the product crates: format, lint, build, tests and guards.
# The guards keep GTK and Linux dependencies out of the engine and controller
# (AD-17), keep directory reads in the shell (AD-9) and require platform facts
# to be injected into the controller (AD-22).
#
# `scripts/check.sh guard` runs the guards and each standalone check.
set -euo pipefail

cd "$(dirname "$0")/.."

MODE=${1:-all}
case "$MODE" in
    all | guard) ;;
    *)
        echo "usage: $0 [guard]" >&2
        exit 2
        ;;
esac

PRODUCT=(addict-core addict-platform addict-dictionary addict-deepl addict-engine addict-controller addict-app)
GUARDED=(addict-core addict-platform addict-dictionary addict-deepl addict-engine addict-controller)
PKG_ARGS=()
for crate in "${PRODUCT[@]}"; do
    PKG_ARGS+=(-p "$crate")
done

# GTK, its supporting libraries and their -sys crates.
GTK='gtk4|gtk4-[a-z0-9-]+|gdk4|gdk4-[a-z0-9-]+|gsk4|gsk4-[a-z0-9-]+|gio|gio-sys|glib|glib-[a-z0-9-]+|gobject-sys|pango|pango-sys|gdk-pixbuf|gdk-pixbuf-sys|cairo-rs|cairo-sys-rs|graphene-rs|graphene-sys'
# The Linux adapters and what only they bring: the portals, D-Bus, the tray.
PLATFORM='addict-platform|ashpd|zbus|ksni'
# The Swift bridge stays above the controller.
UNIFFI='uniffi|uniffi[-_][a-z0-9_-]+'

# The packages `crate` must not reach, as one anchored pattern.
forbidden() {
    case "$1" in
        addict-engine|addict-controller) echo "^($GTK|$PLATFORM|$UNIFFI)$" ;;
        *) echo "^($GTK)$" ;;
    esac
}

if [[ "$MODE" == all ]]; then
    echo "==> fmt"
    cargo fmt "${PKG_ARGS[@]}" -- --check

    echo "==> clippy"
    cargo clippy "${PKG_ARGS[@]}" --all-targets -- -D warnings

    # Without the dev-dependencies and their features: the product must build
    # as it is shipped, where the sample dictionary of the tests does not exist.
    echo "==> product build"
    cargo check "${PKG_ARGS[@]}"

    echo "==> test"
    cargo test "${PKG_ARGS[@]}"
fi

echo "==> dependency guard"
for crate in "${GUARDED[@]}"; do
    # Own assignment: a cargo tree failure aborts the script (set -e).
    tree=$(cargo tree -p "$crate" --prefix none --edges normal,build,dev --target all --all-features)
    leaked=$(printf '%s\n' "$tree" | awk '{print $1}' | grep -E "$(forbidden "$crate")" | sort -u || true)
    if [[ -n "$leaked" ]]; then
        echo "error: $crate depends on forbidden packages:" >&2
        printf '  %s\n' $leaked >&2
        exit 1
    fi
    echo "$crate: ok"
done

echo "==> directory environment and platform predicate guards"
python3 - <<'PYTHON'
from pathlib import Path
import re
import sys
import tomllib

# Strings and comments are single tokens so their braces cannot end a test
# item. Raw strings can also be the argument to an environment reader.
TOKEN = re.compile(r"""
    //[^\n]* | /\*
    | (?:b|c)?r(?P<hashes>\#*)"(?s:.*?)"(?P=hashes)
    | (?:b|c)?"(?:[^"\\]|(?s:\\.))*"
    | b?'(?:[^'\\\n]|\\(?:u\{[\da-fA-F_]+\}|x[\da-fA-F]{2}|.))'
    | (?:r\#)?\w+ | :: | -> | [^\s]
""", re.VERBOSE)
KEY = re.compile(r'(?:r#*)?"(HOME|XDG_\w*)"#*')
BLOCK_COMMENT = re.compile(r"/\*|\*/")
OPEN = {"(": ")", "[": "]", "{": "}"}


def tokenize(source):
    end = 0
    while match := TOKEN.search(source, end):
        end = match.end()
        if match[0] == "/*":
            depth = 1
            while depth and (comment := BLOCK_COMMENT.search(source, end)):
                depth += 1 if comment[0] == "/*" else -1
                end = comment.end()
        elif not match[0].startswith("//"):
            yield match


def groups(tokens):
    ends, stack = {}, []
    for index, token in enumerate(tokens):
        if token in OPEN:
            stack.append(index)
        elif token in OPEN.values():
            ends[stack.pop()] = index
    return ends


PLATFORM_KEYS = {"target_os", "target_family", "target_vendor", "target_env"}
PLATFORM_FAMILIES = {"unix", "windows"}


def platform_predicates(tokens, ends):
    # Keys are rejected anywhere; the bare families only inside cfg(...),
    # cfg!(...) and cfg_attr(...).
    for index, token in enumerate(tokens):
        name = token.removeprefix("r#")
        if name in PLATFORM_KEYS and tokens[index + 1:index + 2] == ["="]:
            yield index
        if name not in ("cfg", "cfg_attr"):
            continue
        opening = index + 2 if tokens[index + 1:index + 2] == ["!"] else index + 1
        if tokens[opening:opening + 1] != ["("] or opening not in ends:
            continue
        for inner in range(opening + 1, ends[opening]):
            if (tokens[inner].removeprefix("r#") in PLATFORM_FAMILIES
                    and tokens[inner - 1] in ("(", ",") and tokens[inner + 1] in (")", ",")):
                yield inner


def after_item(tokens, ends, index):
    # Skip only the item under the attribute and resume at its sibling.
    while index < len(tokens):
        token = tokens[index]
        if token == "{":
            return ends[index] + 1
        if token == ";":
            return index + 1
        index = ends.get(index, index) + 1
    return index


def cfg_without_test(tokens, ends, start, stop):
    """Evaluate cfg with test disabled and other predicates unknown."""
    name = tokens[start]
    if start + 1 == stop:
        return False if name == "test" else None
    if name not in {"all", "any", "not"} or tokens[start + 1] != "(":
        return None
    values = []
    index = start + 2
    while index < stop - 1:
        end = index
        while end < stop - 1 and tokens[end] != ",":
            end = ends.get(end, end) + 1
        values.append(cfg_without_test(tokens, ends, index, end))
        index = end + 1
    if name == "all":
        return False if False in values else True if None not in values else None
    if name == "any":
        return True if True in values else False if None not in values else None
    return not values[0] if len(values) == 1 and values[0] is not None else None


def test_attribute(tokens, ends, start, stop):
    attr = tokens[start:stop]
    if len(attr) >= 4 and attr[:2] == ["cfg", "("]:
        return cfg_without_test(tokens, ends, start + 2, stop - 1) is False
    return attr == ["test"] or attr[:3] == ["tokio", "::", "test"]


def read_key(tokens, ends, index):
    name = tokens[index]
    arg = index + 1
    if name in {"var", "var_os"}:
        if tokens[arg:arg + 2] == ["::", "<"]:
            arg += 2
            depth = 1
            while arg < len(tokens) and depth:
                depth += int(tokens[arg] == "<") - int(tokens[arg] == ">")
                arg = ends.get(arg, arg) + 1
        if tokens[arg:arg + 1] != ["("]:
            return None
    elif name in {"env", "option_env"} and tokens[arg:arg + 1] == ["!"]:
        arg += 1
        if arg not in ends:
            return None
    else:
        return None
    arg += 1
    close = None
    while tokens[arg] == "(":
        if close is not None and ends[arg] != close - 1:
            return None
        close = ends[arg]
        arg += 1
    if close is None or arg + 1 == close:
        return KEY.fullmatch(tokens[arg])
    return None


def module_name(tokens, ends, index):
    if tokens[index:index + 1] == ["pub"]:
        index += 1
        if tokens[index:index + 1] == ["("]:
            index = ends[index] + 1
    if tokens[index:index + 1] == ["mod"] and tokens[index + 2:index + 3] in ([";"], ["{"]):
        return tokens[index + 1].removeprefix("r#"), index + 2
    return None


def inspect(path, module_dir):
    source = path.read_text(encoding="utf-8")
    matches = list(tokenize(source))
    tokens = [match[0] for match in matches]
    ends = groups(tokens)
    reads = []
    platforms = sorted({
        source.count("\n", 0, matches[index].start()) + 1
        for index in platform_predicates(tokens, ends)
    })
    dependencies = []
    test_modules = []

    def record_read(index):
        if key := read_key(tokens, ends, index):
            line = source.count("\n", 0, matches[index].start()) + 1
            reads.append((line, key[1]))

    def scan(index, stop, base, path_base, test_only):
        while index < stop:
            # All consecutive outer attributes apply to the same item.
            item_test = False
            module_path = None
            attributes = index
            while tokens[index:index + 2] == ["#", "["]:
                end = ends[index + 1]
                item_test |= test_attribute(tokens, ends, index + 2, end)
                if tokens[index + 2:index + 4] == ["path", "="]:
                    module_path = tokens[index + 4].split('"', 1)[1].rsplit('"', 1)[0]
                index = end + 1
            if index == stop:
                break
            if not test_only and not item_test:
                for attribute_token in range(attributes, index):
                    record_read(attribute_token)

            if tokens[index:index + 3] == ["#", "!", "["]:
                end = ends[index + 2]
                if test_attribute(tokens, ends, index + 3, end):
                    enclosing = (close for opening, close in ends.items()
                                 if tokens[opening] == "{" and opening < index < close)
                    close = min(enclosing, default=stop)
                    if close < stop:
                        index = close + 1
                        continue
                    test_only = True
                if not test_only:
                    for attribute_token in range(index + 3, end):
                        record_read(attribute_token)
                index = end + 1
                continue

            if module := module_name(tokens, ends, index):
                name, body = module
                is_test = test_only or item_test
                child_base = base / name if module_path is None else path_base / module_path
                if tokens[body] == "{":
                    scan(body + 1, ends[body], child_base, child_base, is_test)
                    index = ends[body] + 1
                else:
                    # A path attribute starts the file's module at its parent,
                    # even if the file is not named mod.rs.
                    targets = ((child_base, child_base.parent),) if module_path is not None else (
                        (base / f"{name}.rs", child_base), (child_base / "mod.rs", child_base),
                    )
                    links = test_modules if is_test else dependencies
                    links.extend((target.resolve(), child_dir.resolve())
                                 for target, child_dir in targets if target.is_file())
                    index = body + 1
                continue

            if item_test:
                index = after_item(tokens, ends, index)
                continue
            if not test_only:
                record_read(index)
            index += 1

    scan(0, len(tokens), module_dir, path.parent, False)
    return reads, platforms, dependencies, test_modules


def linked_files(initial, links):
    found = set(initial)
    pending = list(found)
    while pending:
        for path in links.get(pending.pop(), ()):
            if path not in found:
                found.add(path)
                pending.append(path)
    return found


failed = False
for crate in ("addict-core", "addict-dictionary", "addict-deepl", "addict-engine", "addict-controller"):
    root = Path("crates") / crate
    files = [*root.glob("build.rs"), *root.glob("src/**/*.rs"), *root.glob("examples/**/*.rs")]
    if crate == "addict-controller":
        files.extend(root.glob("tests/**/*.rs"))
        manifest = root / "Cargo.toml"
        with manifest.open("rb") as file:
            # Every target table is a platform difference.
            for target in tomllib.load(file).get("target", {}):
                print(f"{manifest}: inject platform facts instead of using target.{target}", file=sys.stderr)
                failed = True
    crate_roots = {
        root / "build.rs", root / "src/lib.rs", root / "src/main.rs",
        *root.glob("src/bin/*.rs"), *root.glob("examples/*.rs"),
    }
    if crate == "addict-controller":
        crate_roots.update(root.glob("tests/*.rs"))
    reports, links, test_modules, seen = {}, {}, set(), set()
    pending = [
        (path, path.parent if path in crate_roots or path.name == "mod.rs" else path.with_suffix(""))
        for path in sorted(files, reverse=True)
    ]
    while pending:
        path, base = pending.pop()
        resolved = path.resolve()
        location = (resolved, base.resolve())
        if location in seen:
            continue
        seen.add(location)
        reads, platforms, dependencies, test_targets = inspect(path, base)
        if resolved not in reports:
            reports[resolved] = (path, reads)
            if crate == "addict-controller":
                for line in platforms:
                    print(f"{path}:{line}: inject platform facts instead of using a platform cfg", file=sys.stderr)
                    failed = True
        pending.extend(dependencies)
        pending.extend(test_targets)
        links.setdefault(resolved, set()).update(target for target, _ in dependencies)
        test_modules.update(target for target, _ in test_targets)

    # A file reached from a production module is checked even if test code
    # also references it via a second declaration.
    test_files = linked_files(test_modules, links)
    production = linked_files((reports.keys() - test_files) | {path.resolve() for path in crate_roots},
                              links)
    for resolved, (path, reads) in reports.items():
        if resolved in production:
            for line, name in reads:
                print(f"{path}:{line}: read {name} in the platform shell", file=sys.stderr)
                failed = True
sys.exit(failed)
PYTHON

# Features must be available without feature unification in a workspace build.
for crate in addict-engine addict-controller; do
    echo "==> $crate alone"
    cargo check -p "$crate"
done

echo "ok"
