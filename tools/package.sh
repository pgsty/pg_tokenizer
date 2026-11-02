#!/usr/bin/env bash
set -xeu

printf "SEMVER = ${SEMVER}\n"
printf "VERSION = ${VERSION}\n"
printf "ARCH = ${ARCH}\n"
printf "PLATFORM = ${PLATFORM}\n"

cargo build --lib --features pg$VERSION --release
cargo pgrx schema --features pg$VERSION --out ./target/schema.sql

mkdir -p ./build/zip
[[ -d ./sql/upgrade ]] && cp -a ./sql/upgrade/. ./build/zip/
cp ./target/schema.sql ./build/zip/pg_tokenizer--$SEMVER.sql
sed -e "s/@CARGO_VERSION@/$SEMVER/g" <./pg_tokenizer.control >./build/zip/pg_tokenizer.control
cp ./target/release/libpg_tokenizer.so ./build/zip/pg_tokenizer.so
zip ./build/postgresql-${VERSION}-pg-tokenizer_${SEMVER}_${ARCH}-linux-gnu.zip -j ./build/zip/*

mkdir -p ./build/deb
mkdir -p ./build/deb/DEBIAN
mkdir -p ./build/deb/usr/share/postgresql/$VERSION/extension/
mkdir -p ./build/deb/usr/lib/postgresql/$VERSION/lib/
mkdir -p ./build/deb/usr/share/doc/postgresql-$VERSION-pg-tokenizer/
for file in $(ls ./build/zip/*.sql | xargs -n 1 basename); do
    cp ./build/zip/$file ./build/deb/usr/share/postgresql/$VERSION/extension/$file
done
for file in $(ls ./build/zip/*.control | xargs -n 1 basename); do
    cp ./build/zip/$file ./build/deb/usr/share/postgresql/$VERSION/extension/$file
done
for file in $(ls ./build/zip/*.so | xargs -n 1 basename); do
    cp ./build/zip/$file ./build/deb/usr/lib/postgresql/$VERSION/lib/$file
done
echo "Format: https://www.debian.org/doc/packaging-manuals/copyright-format/1.0/
Source: https://github.com/pgsty/pg_tokenizer

Files: *
Copyright: 2025-2026 TensorChord Inc.
           2026 PGSTY PTE. LTD.
License: Apache-2.0
$(sed -e 's/^$/./g' -e 's/^/ /g' ./LICENSE)" \
    >./build/deb/usr/share/doc/postgresql-$VERSION-pg-tokenizer/copyright
echo "Package: postgresql-${VERSION}-pg-tokenizer
Version: ${SEMVER}-1
Section: database
Priority: optional
Architecture: ${PLATFORM}
Maintainer: Ruohang Feng <rh@vonng.com>
Description: Tokenizer plugin for PostgreSQL
Homepage: https://vchord.pgsty.com/
License: Apache-2.0" \
    >./build/deb/DEBIAN/control
(cd ./build/deb && md5sum usr/share/postgresql/$VERSION/extension/* usr/lib/postgresql/$VERSION/lib/* usr/share/doc/postgresql-$VERSION-pg-tokenizer/*) >./build/deb/DEBIAN/md5sums
dpkg-deb --root-owner-group -Zxz --build ./build/deb/ ./build/postgresql-${VERSION}-pg-tokenizer_${SEMVER}-1_${PLATFORM}.deb

ls ./build
