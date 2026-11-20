# A thin wrapper around cargo and scripts/install.sh, for the usual
# `make && sudo make install`. The features to build come from FEATURES:
# set it in config.mk (optional, not tracked) or on the command line:
#
#   make FEATURES="theme multimonitor"
#
# Install as in scripts/install.sh: `sudo make install` for every user,
# under /usr/local; `make install` just for you, under ~/.local.
# Packagers can pass PREFIX, DESTDIR and SYSTEMD:
#
#   make install PREFIX=/usr DESTDIR=pkg

-include config.mk

# Every feature by default; empty for the minimal binary (black screen only).
FEATURES ?= multimonitor ignore-mouse theme
CARGO ?= cargo

# Passed on to scripts/install.sh, which reads them from the environment.
export PREFIX DESTDIR SYSTEMD

.PHONY: all build install uninstall check clean

all: build

# Cargo decides what needs rebuilding, so this always runs it.
build:
	$(CARGO) build --release --no-default-features --features "$(FEATURES)"

# Doesn't build: `sudo make install` must not run cargo as root.
install:
	scripts/install.sh $(INSTALL_FLAGS)

uninstall:
	scripts/install.sh --uninstall

check:
	scripts/check.sh

clean:
	$(CARGO) clean
