#!/usr/bin/env bash
set -xeu

version=$1

sudo apt-get update
sudo apt-get remove -y '^postgres.*' '^libpq.*'
sudo apt-get purge -y '^postgres.*' '^libpq.*'
sudo apt-get install -y postgresql-common
sudo /usr/share/postgresql-common/pgdg/apt.postgresql.org.sh -y
sudo apt-get install -y postgresql-server-dev-${version}
sudo apt-get install -y postgresql-${version} python3-venv zip dpkg-dev

echo "local all all trust" | sudo tee /etc/postgresql/${version}/main/pg_hba.conf
echo "host all all 127.0.0.1/32 trust" | sudo tee -a /etc/postgresql/${version}/main/pg_hba.conf
echo "host all all ::1/128 trust" | sudo tee -a /etc/postgresql/${version}/main/pg_hba.conf
sudo -iu postgres createuser -s -r $USER
sudo -iu postgres createdb -O $USER $USER
sudo -iu postgres psql -c "ALTER SYSTEM SET shared_preload_libraries = 'pg_tokenizer.so';"
sudo -iu postgres psql -c "ALTER SYSTEM SET max_prepared_transactions = 10;"
sudo systemctl stop postgresql

PGRX_VERSION=$(grep -o 'pgrx = "=[^"]*' Cargo.toml | cut -d = -f 3)
RUSTFLAGS="" cargo install --locked cargo-pgrx --version "$PGRX_VERSION"
cargo pgrx init --pg${version}=$(which pg_config)

curl -fsSL https://github.com/risinglightdb/sqllogictest-rs/releases/download/v0.26.4/sqllogictest-bin-v0.26.4-$(uname -m)-unknown-linux-musl.tar.gz | tar -xOzf - ./sqllogictest | install -m 755 /dev/stdin /usr/local/bin/sqllogictest
