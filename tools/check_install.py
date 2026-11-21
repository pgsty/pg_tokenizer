"""Verify installed extension library symbols and generated install SQL parity."""

import argparse
import difflib
import os
from pathlib import Path
import platform
import re
import subprocess
import tomllib

ROOT = Path(__file__).resolve().parent.parent
BLOCK_RE = re.compile(
    r"/\* <begin connected objects> \*/\n(.*?)/\* </end connected objects> \*/",
    re.DOTALL,
)
SOURCE_LINE_RE = re.compile(r"^-- src/[^\n]+:\d+\n", re.MULTILINE)
SYMBOL_RE = re.compile(r"'MODULE_PATHNAME'\s*,\s*'([A-Za-z0-9_]+)'")


def capture(*args):
    return subprocess.check_output(args, text=True).strip()


def parse_blocks(sql_text, label):
    outside = BLOCK_RE.sub("", sql_text).strip()
    if outside:
        raise ValueError(f"Unexpected SQL outside connected-object blocks in {label}: {outside[:120]!r}")
    blocks = [SOURCE_LINE_RE.sub("", m.group(1)).strip() for m in BLOCK_RE.finditer(sql_text)]
    if len(blocks) < 2:
        raise ValueError(f"Expected header and SQL object blocks in {label}")
    return blocks


def verify_schema_sql(expected_text, actual_text, expected_label="checked-in SQL", actual_label="generated SQL"):
    expected = parse_blocks(expected_text, expected_label)
    actual = parse_blocks(actual_text, actual_label)
    if expected[0] != actual[0]:
        raise ValueError(f"Header block mismatch between {expected_label} and {actual_label}")
    if expected[-1] != actual[-1]:
        diff = "\n".join(difflib.unified_diff(
            expected[-1].splitlines(), actual[-1].splitlines(),
            fromfile=f"{expected_label} (finalize)", tofile=f"{actual_label} (finalize)", lineterm="",
        ))
        raise ValueError(f"Finalize SQL block mismatch:\n{diff}")
    if sorted(expected) != sorted(actual):
        diff = "\n".join(difflib.unified_diff(
            "\n\n".join(sorted(expected)).splitlines(),
            "\n\n".join(sorted(actual)).splitlines(),
            fromfile=expected_label, tofile=actual_label, lineterm="",
        ))
        raise ValueError(f"Install SQL drift detected:\n{diff}")


def referenced_symbols(sql_files):
    symbols = set()
    for path in sql_files:
        symbols.update(SYMBOL_RE.findall(Path(path).read_text()))
    if not symbols:
        raise ValueError("No MODULE_PATHNAME C symbols found in SQL files")
    return sorted(symbols)


def exported_symbols(library):
    if platform.system() == "Darwin":
        output = capture("nm", "-gU", str(library))
        return {line.split()[-1].removeprefix("_") for line in output.splitlines() if len(line.split()) >= 3}
    output = capture("nm", "-D", "--defined-only", str(library))
    return {line.split()[-1] for line in output.splitlines() if len(line.split()) >= 3}


def find_library(pkglibdir):
    for suffix in (".so", ".dylib"):
        candidate = Path(pkglibdir) / f"pg_tokenizer{suffix}"
        if candidate.is_file():
            return candidate
    raise FileNotFoundError(f"pg_tokenizer shared library not found in {pkglibdir}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--pg-config", default=os.environ.get("PG_CONFIG", "pg_config"))
    args = parser.parse_args()

    version = tomllib.loads((ROOT / "Cargo.toml").read_text())["package"]["version"]
    pkglibdir = capture(args.pg_config, "--pkglibdir")
    sharedir = capture(args.pg_config, "--sharedir")

    expected_sql = ROOT / "sql" / "install" / f"pg_tokenizer--{version}.sql"
    installed_sql = Path(sharedir) / "extension" / f"pg_tokenizer--{version}.sql"
    verify_schema_sql(
        expected_sql.read_text(),
        installed_sql.read_text(),
        str(expected_sql.relative_to(ROOT)),
        str(installed_sql),
    )

    library = find_library(pkglibdir)
    exported = exported_symbols(library)
    sql_files = sorted((ROOT / "sql" / "install").glob("pg_tokenizer--*.sql")) + sorted(
        (ROOT / "sql" / "upgrade").glob("pg_tokenizer--*--*.sql")
    )
    referenced = referenced_symbols(sql_files)
    required = {"_PG_init", "Pg_magic_func"}
    for sym in referenced:
        required.add(sym)
        required.add(f"pg_finfo_{sym}")
    missing = sorted(required - exported)
    if missing:
        raise SystemExit(f"Missing exported C symbols in {library}: {', '.join(missing)}")
    print(
        f"Verified {installed_sql.name} parity and {len(referenced)} SQL C symbols "
        f"({len(required)} exports) against {library}"
    )


if __name__ == "__main__":
    main()
