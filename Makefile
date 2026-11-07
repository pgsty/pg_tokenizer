PG_CONFIG ?= pg_config
PYTHON ?= python3
FEATURES ?= lindera-ipadic
PGXN_BUILD_DIR ?= build/pgxn
DIST_DIR ?= build/dist
REF ?= HEAD

.PHONY: all build install meta dist

all: build

build:
	$(PYTHON) tools/pgxn.py build --pg-config "$(PG_CONFIG)" --build-dir "$(PGXN_BUILD_DIR)" --features "$(FEATURES)"

install:
	$(PYTHON) tools/pgxn.py install --pg-config "$(PG_CONFIG)" --build-dir "$(PGXN_BUILD_DIR)" --destdir "$(DESTDIR)"

meta:
	$(PYTHON) tools/pgxn.py meta --output META.json

dist:
	$(PYTHON) tools/pgxn.py dist --ref "$(REF)" --output-dir "$(DIST_DIR)"
