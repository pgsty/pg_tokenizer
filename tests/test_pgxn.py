"""Unit tests for tools/pgxn.py. They need no PostgreSQL, Rust or network.

Run with: python3 tests/test_pgxn.py
"""

import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest import mock

TOOLS = Path(__file__).resolve().parent.parent / "tools" / "pgxn.py"
spec = importlib.util.spec_from_file_location("pgxn", TOOLS)
pgxn = importlib.util.module_from_spec(spec)
spec.loader.exec_module(pgxn)

PATHS = {"version": "PostgreSQL 18.0", "pkglibdir": "/lib", "sharedir": "/share"}


class InstallTests(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name)
        self.build = self.root / "build"
        self.dest = self.root / "dest"
        patches = [
            mock.patch.object(pgxn, "metadata", return_value=("1.1.0", "{}")),
            mock.patch.object(pgxn, "pg_paths", return_value=PATHS),
            mock.patch.object(pgxn, "capture", return_value="cargo-pgrx 0.19.3"),
        ]
        for patch in patches:
            patch.start()
            self.addCleanup(patch.stop)

    def stage(self, files, package="package-test"):
        for name, content in files.items():
            path = self.build / package / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(content)
            path.chmod(0o755 if name.endswith(".so") else 0o644)
        manifest = {"version": "1.1.0", "paths": PATHS, "package": package,
                    "files": {name: hashlib.sha256(content).hexdigest() for name, content in files.items()}}
        (self.build / "manifest.json").write_text(json.dumps(manifest))

    def install(self):
        pgxn.install(argparse.Namespace(pg_config="pg_config", build_dir=self.build, destdir=str(self.dest)))

    def build_args(self):
        return argparse.Namespace(pg_config="pg_config", build_dir=self.build, features="lindera-ipadic")

    def test_install_replaces_files_instead_of_rewriting_them(self):
        library = self.dest / "lib/pg_tokenizer.so"
        library.parent.mkdir(parents=True)
        library.write_bytes(b"old library")
        before = library.stat().st_ino
        self.stage({"lib/pg_tokenizer.so": b"new library", "share/extension/pg_tokenizer.control": b"control"})
        with library.open("rb") as running:  # like a process that mapped the old file
            self.install()
            # The old inode is untouched for processes that still use it.
            self.assertEqual(running.read(), b"old library")
        self.assertNotEqual(library.stat().st_ino, before)
        self.assertEqual(library.read_bytes(), b"new library")
        self.assertEqual(library.stat().st_mode & 0o777, 0o755)
        self.assertEqual((self.dest / "share/extension/pg_tokenizer.control").read_bytes(), b"control")
        self.assertEqual(sorted(p.name for p in library.parent.iterdir()), ["pg_tokenizer.so"])

    def test_tampered_stage_installs_nothing(self):
        self.stage({"lib/pg_tokenizer.so": b"library", "share/extension/pg_tokenizer.control": b"control"})
        (self.build / "package-test/share/extension/pg_tokenizer.control").write_bytes(b"changed")
        with self.assertRaisesRegex(ValueError, "Staged file changed"):
            self.install()
        self.assertFalse(self.dest.exists())

    def test_failed_build_discards_previous_stage(self):
        self.stage({"lib/pg_tokenizer.so": b"previous build"})
        failure = subprocess.CalledProcessError(101, ["cargo", "pgrx", "package"])
        with mock.patch.object(pgxn.subprocess, "run", side_effect=failure):
            with self.assertRaises(subprocess.CalledProcessError):
                pgxn.build(self.build_args())
        self.assertFalse((self.build / "manifest.json").exists())
        with self.assertRaisesRegex(FileNotFoundError, "run make first"):
            self.install()
        self.assertFalse(self.dest.exists())

    def test_successful_build_replaces_previous_stage(self):
        self.stage({"lib/pg_tokenizer.so": b"previous build"})

        def package(argv, check):
            out = Path(argv[argv.index("--out-dir") + 1])
            extension = out / PATHS["sharedir"].lstrip("/") / "extension"
            extension.mkdir(parents=True)
            (extension / "pg_tokenizer.control").write_text("default_version = '1.1.0'\n")
            library = out / PATHS["pkglibdir"].lstrip("/") / "pg_tokenizer.so"
            library.parent.mkdir(parents=True)
            library.write_bytes(b"new build")

        cwd = os.getcwd()
        os.chdir(TOOLS.parent.parent)  # build() copies sql/upgrade from the source tree
        self.addCleanup(os.chdir, cwd)
        with mock.patch.object(pgxn.subprocess, "run", side_effect=package):
            pgxn.build(self.build_args())
        manifest = json.loads((self.build / "manifest.json").read_text())
        self.assertNotEqual(manifest["package"], "package-test")
        self.assertFalse((self.build / "package-test").exists())
        self.assertIn("share/extension/pg_tokenizer--1.0.0--1.1.0.sql", manifest["files"])
        self.install()
        self.assertEqual((self.dest / "lib/pg_tokenizer.so").read_bytes(), b"new build")


if __name__ == "__main__":
    unittest.main(verbosity=2)
