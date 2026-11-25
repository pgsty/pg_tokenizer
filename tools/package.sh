#!/usr/bin/env bash
set -xeu

printf "SEMVER = ${SEMVER}\n"
printf "VERSION = ${VERSION}\n"
printf "ARCH = ${ARCH}\n"
printf "PLATFORM = ${PLATFORM}\n"

# Build the release library and its schema together. cargo-pgrx scopes the
# schema-section retention flag to this cdylib instead of every Rust target.
cargo pgrx schema --release --cargo=--locked --features "pg${VERSION} lindera-ipadic" --out ./target/schema.sql

mkdir -p ./build/zip
[[ -d ./sql/upgrade ]] && cp -a ./sql/upgrade/. ./build/zip/
cp ./target/schema.sql ./build/zip/pg_tokenizer--$SEMVER.sql
sed -e "s/@CARGO_VERSION@/$SEMVER/g" <./pg_tokenizer.control >./build/zip/pg_tokenizer.control
cp ./target/release/libpg_tokenizer.so ./build/zip/pg_tokenizer.so
# The tuple-header checks became exported server functions in these minor releases.
pg_dependency="postgresql-${VERSION}"
pg_minimum=$VERSION
nm -D --undefined-only ./build/zip/pg_tokenizer.so > ./target/pg_tokenizer.imports
if grep -Eq '[[:space:]](DatumGetHeapTupleHeader|heap_datum_check)$' ./target/pg_tokenizer.imports; then
    case "$VERSION" in
        15) pg_minimum=15.20 ;;
        16) pg_minimum=16.16 ;;
        17) pg_minimum=17.12 ;;
        18) pg_minimum=18.7 ;;
    esac
    pg_dependency+=" (>= $pg_minimum)"
fi
echo "# Minimum PostgreSQL version for this binary: $pg_minimum" >> ./build/zip/pg_tokenizer.control
ipadic_manifest=$(cargo metadata --locked --format-version 1 --filter-platform "$(rustc --print host-tuple)" \
  --features "pg${VERSION} lindera-ipadic" | python3 -c 'import json, sys
manifest, = (pkg["manifest_path"] for pkg in json.load(sys.stdin)["packages"] if pkg["name"] == "lindera-ipadic")
print(manifest)')
install -m 644 "$(dirname "$ipadic_manifest")/NOTICE.txt" ./build/zip/IPADIC-NOTICE.txt
zip ./build/postgresql-${VERSION}-pg-tokenizer_${SEMVER}_${ARCH}-linux-gnu.zip -j ./build/zip/*

mkdir -p ./build/deb
mkdir -p ./build/deb/DEBIAN
mkdir -p ./build/deb/usr/share/postgresql/$VERSION/extension/
mkdir -p ./build/deb/usr/lib/postgresql/$VERSION/lib/
mkdir -p ./build/deb/usr/share/doc/postgresql-$VERSION-pg-tokenizer/
cp ./build/zip/*.sql ./build/zip/*.control ./build/deb/usr/share/postgresql/$VERSION/extension/
cp ./build/zip/*.so ./build/deb/usr/lib/postgresql/$VERSION/lib/
cp ./build/zip/IPADIC-NOTICE.txt ./build/deb/usr/share/doc/postgresql-$VERSION-pg-tokenizer/
# Derive runtime dependencies from the actual ELF, including libstdc++.
mkdir -p ./build/debian
printf 'Source: pg-tokenizer\n\nPackage: postgresql-%s-pg-tokenizer\nArchitecture: any\nDescription: PostgreSQL tokenizer\n' "$VERSION" > ./build/debian/control
SHLIBS_DEPENDS=$(cd ./build && dpkg-shlibdeps -O -e zip/pg_tokenizer.so | sed -n 's/^shlibs:Depends=//p')
test -n "$SHLIBS_DEPENDS"
echo "Format: https://www.debian.org/doc/packaging-manuals/copyright-format/1.0/
Source: https://github.com/pgsty/pg_tokenizer

Files: *
Copyright: 2025 TensorChord Inc.
           2026 PGSTY PTE. LTD.
License: Apache-2.0
$(sed -e 's/^$/./g' -e 's/^/ /g' ./LICENSE)" \
    >./build/deb/usr/share/doc/postgresql-$VERSION-pg-tokenizer/copyright
echo "Package: postgresql-${VERSION}-pg-tokenizer
Version: ${SEMVER}-1
Section: database
Priority: optional
Architecture: ${PLATFORM}
Depends: ${pg_dependency}, ${SHLIBS_DEPENDS}
Maintainer: Ruohang Feng <rh@vonng.com>
Description: Tokenizer plugin for PostgreSQL
Homepage: https://vchord.pgsty.com/
License: Apache-2.0" \
    >./build/deb/DEBIAN/control
(cd ./build/deb && md5sum usr/share/postgresql/$VERSION/extension/* usr/lib/postgresql/$VERSION/lib/* usr/share/doc/postgresql-$VERSION-pg-tokenizer/*) >./build/deb/DEBIAN/md5sums
dpkg-deb --root-owner-group -Zxz --build ./build/deb/ ./build/postgresql-${VERSION}-pg-tokenizer_${SEMVER}-1_${PLATFORM}.deb

ls ./build
