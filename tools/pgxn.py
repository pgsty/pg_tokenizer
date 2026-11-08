"""Build/install pgrx packages and produce PGXN source archives. Never uploads."""

import argparse
import datetime
import hashlib
import json
import os
from pathlib import Path
import posixpath
import re
import shutil
import subprocess
import tarfile
import tempfile
import tomllib
import zipfile


def capture(*args):
    return subprocess.check_output(args, text=True).strip()


def metadata(read):
    cargo = tomllib.loads(read("Cargo.toml"))
    version = cargo["package"]["version"]
    locked = next(p["version"] for p in tomllib.loads(read("Cargo.lock"))["package"]
                  if p["name"] == "pg_tokenizer")
    if version != locked or not re.fullmatch(r"\d+\.\d+\.\d+", version):
        raise ValueError("Cargo.toml/Cargo.lock must agree on a stable three-part version")
    meta = json.loads(read("META.json.in").replace("@DISTVERSION@", version))
    for extension in meta["provides"].values():
        if extension["version"] != version:
            raise ValueError("PGXN extension and distribution versions differ")
        read(extension["file"])
        read(extension["docfile"])
    return version, json.dumps(meta, indent=2, ensure_ascii=False) + "\n"


def pg_paths(pg_config):
    info = {key: capture(pg_config, "--" + key) for key in ("version", "pkglibdir", "sharedir")}
    major = int(info["version"].split()[1].split(".")[0])
    if major not in range(14, 19):
        raise ValueError("This release supports PostgreSQL 14 through 18")
    return info


def build(args):
    version, _ = metadata(lambda name: Path(name).read_text())
    paths = pg_paths(args.pg_config)
    if capture("cargo", "pgrx", "--version") != "cargo-pgrx 0.19.3":
        raise ValueError("Install cargo-pgrx 0.19.3 to match this extension's pgrx version")
    root = args.build_dir.resolve()
    root.mkdir(parents=True, exist_ok=True)
    # A failed build must not leave the previous manifest for `make install`
    # to install. Its staging directory is removed once a new build succeeds.
    manifest_path = root / "manifest.json"
    previous = None
    if manifest_path.exists():
        try:
            previous = json.loads(manifest_path.read_text()).get("package")
        except (ValueError, AttributeError):
            pass
        manifest_path.unlink()
    # Only ever remove a staging directory this tool created inside root.
    if not (isinstance(previous, str) and previous.startswith("package-") and Path(previous).name == previous):
        previous = None
    package = Path(tempfile.mkdtemp(prefix="package-", dir=root))
    try:
        subprocess.run(["cargo", "pgrx", "package", "--pg-config", args.pg_config,
                        "--out-dir", str(package), "--features", args.features,
                        "--cargo=--locked"], check=True)
        # cargo-pgrx scans sql/ only; this repository keeps migrations below it.
        extension_dir = package / paths["sharedir"].lstrip("/") / "extension"
        for script in sorted(Path("sql/upgrade").glob("pg_tokenizer--*--*.sql")):
            shutil.copy2(script, extension_dir / script.name)
        files = {}
        for path in sorted(package.rglob("*")):
            if path.is_file():
                files[str(path.relative_to(package))] = hashlib.sha256(path.read_bytes()).hexdigest()
        control = package / paths["sharedir"].lstrip("/") / "extension/pg_tokenizer.control"
        if f"default_version = '{version}'" not in control.read_text():
            raise ValueError("Staged control version does not match Cargo")
        manifest = {"version": version, "paths": paths, "package": package.name, "files": files}
        staged = root / "manifest.json.tmp"
        staged.write_text(json.dumps(manifest, indent=2) + "\n")
        os.replace(staged, manifest_path)
    except BaseException:
        shutil.rmtree(package)
        raise
    if previous and previous != package.name and (root / previous).is_dir():
        shutil.rmtree(root / previous)
    print(f"Staged pg_tokenizer {version} for {paths['version']} in {package}")


def replace_file(source, destination):
    """Install a file by renaming a complete copy over the destination.

    A running server maps the preloaded library; rewriting that file in place
    can crash every process using it. Like install(1) and cargo-pgrx, never
    truncate an installed file: the old inode stays intact for running
    processes until they exit, and readers never see a partial file.
    """
    destination.parent.mkdir(parents=True, exist_ok=True)
    descriptor, temporary = tempfile.mkstemp(prefix=f".{destination.name}.", dir=destination.parent)
    os.close(descriptor)
    try:
        shutil.copy2(source, temporary)
        os.replace(temporary, destination)
    except BaseException:
        Path(temporary).unlink(missing_ok=True)
        raise


def install(args):
    version, _ = metadata(lambda name: Path(name).read_text())
    root = args.build_dir.resolve()
    manifest_path = root / "manifest.json"
    if not manifest_path.exists():
        raise FileNotFoundError(f"No staged package in {root}; run make first")
    manifest = json.loads(manifest_path.read_text())
    if manifest["version"] != version or manifest["paths"] != pg_paths(args.pg_config):
        raise ValueError("Staged package differs from the selected version/PostgreSQL; run make first")
    package = root / manifest["package"]
    # Verify every staged input before copying any file into the installation.
    for name, digest in manifest["files"].items():
        if hashlib.sha256((package / name).read_bytes()).hexdigest() != digest:
            raise ValueError(f"Staged file changed: {name}")
    prefix = Path(args.destdir).resolve() if args.destdir else Path("/")
    for name in manifest["files"]:
        replace_file(package / name, prefix / name)
    print(f"Installed {len(manifest['files'])} files under {prefix}")


def dist(args):
    commit = capture("git", "rev-parse", "--verify", args.ref + "^{commit}")
    with tempfile.TemporaryDirectory(prefix="pg-tokenizer-dist-") as directory:
        archive = Path(directory) / "source.tar"
        subprocess.run(["git", "archive", "--format=tar", "--output", str(archive), commit], check=True)
        with tarfile.open(archive) as source:
            def read(name):
                return source.extractfile(name).read().decode()
            version, meta = metadata(read)
            args.output_dir.mkdir(parents=True, exist_ok=True)
            path = args.output_dir / f"pg_tokenizer-{version}.zip"
            if path.exists():
                raise FileExistsError(f"Refusing to overwrite {path}")
            prefix = f"pg_tokenizer-{version}/"
            timestamp = int(capture("git", "show", "-s", "--format=%ct", commit))
            stamp = datetime.datetime.fromtimestamp(max(timestamp, 315532800), datetime.timezone.utc).timetuple()[:6]
            with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as target:
                def write(name, content, mode):
                    info = zipfile.ZipInfo(prefix + name, stamp)
                    info.external_attr = (0o100000 | mode) << 16
                    info.compress_type = zipfile.ZIP_DEFLATED
                    target.writestr(info, content)
                for member in source:
                    entry = member
                    if member.issym():
                        # SQL aliases are repository-relative symlinks. Store
                        # their contents for ZIP extractors without link support.
                        name = posixpath.normpath(posixpath.join(posixpath.dirname(member.name), member.linkname))
                        if name.startswith(("/", "../")):
                            raise ValueError(f"Source symlink escapes archive: {member.name}")
                        entry = source.getmember(name)
                        if not entry.isfile():
                            raise ValueError(f"Source symlink must target a regular file: {member.name}")
                    elif not member.isfile():
                        if member.isdir():
                            continue
                        raise ValueError(f"Unexpected non-regular source entry: {member.name}")
                    content = source.extractfile(entry).read()
                    if member.name == "pg_tokenizer.control":
                        content = content.replace(b"@CARGO_VERSION@", version.encode())
                    write(member.name, content, entry.mode)
                write("META.json", meta.encode(), 0o644)
            digest = hashlib.sha256(path.read_bytes()).hexdigest()
            path.with_suffix(".zip.sha256").write_text(f"{digest}  {path.name}\n")
            print(f"{commit}: {path} (SHA256 {digest})")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    for name in ("build", "install"):
        command = commands.add_parser(name)
        command.add_argument("--pg-config", default=os.environ.get("PG_CONFIG", "pg_config"))
        command.add_argument("--build-dir", type=Path, default=Path("build/pgxn"))
        if name == "build":
            command.add_argument("--features", default="lindera-ipadic")
        else:
            command.add_argument("--destdir", default="")
    command = commands.add_parser("meta")
    command.add_argument("--output", type=Path, default=Path("META.json"))
    command = commands.add_parser("dist")
    command.add_argument("--ref", default="HEAD")
    command.add_argument("--output-dir", type=Path, default=Path("build/dist"))
    args = parser.parse_args()
    if args.command == "meta":
        _, meta = metadata(lambda name: Path(name).read_text())
        args.output.write_text(meta)
    else:
        {"build": build, "install": install, "dist": dist}[args.command](args)


if __name__ == "__main__":
    main()
