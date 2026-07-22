#!/bin/sh
set -eu

fail() {
  printf 'savestate uninstaller: %s\n' "$1" >&2
  exit 1
}

if [ -n "${SAVESTATE_INSTALL_DIR:-}" ]; then
  install_dir="$SAVESTATE_INSTALL_DIR"
elif [ -n "${HOME:-}" ]; then
  install_dir="$HOME/.local/bin"
else
  fail "HOME is not set; set SAVESTATE_INSTALL_DIR to the directory used during installation"
fi

case "$install_dir" in
  /*) ;;
  *) fail "SAVESTATE_INSTALL_DIR must be an absolute path" ;;
esac

binary="$install_dir/savestate"

if [ -d "$binary" ]; then
  fail "refusing to remove a directory at $binary"
fi

if [ -e "$binary" ] || [ -L "$binary" ]; then
  rm -f "$binary"
  printf 'Removed Savestate from %s\n' "$binary"
else
  printf 'Savestate was not found at %s\n' "$binary"
  if command -v savestate >/dev/null 2>&1; then
    printf 'Another installation is still available at %s\n' "$(command -v savestate)"
  fi
fi

printf '\nProject checkpoints and configuration were not removed.\n'
printf 'For Cargo installations, run: cargo uninstall savestate\n'
