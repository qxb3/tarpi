
#!/bin/bash

# Build tarpi for Android (Termux) with the NDK.
#
# Usage:
#   ./arm_build.sh                         # Interactive architecture selection
#   ./arm_build.sh aarch64                  # Build one architecture (release)
#   ./arm_build.sh armv7 aarch64            # Build several architectures
#   ./arm_build.sh all                      # Build all architectures
#   ./arm_build.sh --debug aarch64          # Build debug profile
#   ./arm_build.sh --debug all              # Build all in debug profile
#   ./arm_build.sh --list                   # Show supported architectures
#
# NDK=/path/to/android-ndk-rXX and API=24 can be set in the environment.

set -u

NDK="${NDK:-$PWD/android-ndk-r30}"
API="${API:-24}"
BIN="$NDK/toolchains/llvm/prebuilt/linux-x86_64/bin"

DEBUG=0
PROFILE="release"
BUILD_FLAG="--release"

# ---- Parse global flags

args=()

for arg in "$@"; do
  case "$arg" in
    --debug)
      DEBUG=1
      PROFILE="debug"
      BUILD_FLAG=""
      ;;
    *)
      args+=("$arg")
      ;;
  esac
done

set -- "${args[@]}"

# ---- Architecture definitions
#
# name   aliases...            rust target               clang prefix

ARCHS=(
  "armv7   armv7a arm armeabi-v7a   armv7-linux-androideabi   armv7a-linux-androideabi"
  "aarch64 arm64 arm64-v8a          aarch64-linux-android     aarch64-linux-android"
  "x86_64  x64 amd64                x86_64-linux-android      x86_64-linux-android"
  "i686    x86 i386                 i686-linux-android        i686-linux-android"
)

# Resolve architecture name or alias.
resolve() {
  local want="$1" line words n
  for line in "${ARCHS[@]}"; do
    read -r -a words <<<"$line"
    n=${#words[@]}

    local target="${words[$((n - 2))]}"
    local clang="${words[$((n - 1))]}"

    local w
    for w in "${words[@]:0:$((n - 2))}"; do
      if [ "$w" = "$want" ]; then
        echo "${words[0]} $target $clang"
        return 0
      fi
    done
  done

  return 1
}

list_archs() {
  local line words n

  echo "Supported architectures:"

  for line in "${ARCHS[@]}"; do
    read -r -a words <<<"$line"
    n=${#words[@]}

    printf '  %-8s (also: %s) -> %s\n' \
      "${words[0]}" \
      "${words[*]:1:$((n - 3))}" \
      "${words[$((n - 2))]}"
  done

  echo "  all      every architecture above"
}

build() {
  local name="$1"
  local target="$2"
  local clang="$3"

  local cc="$BIN/${clang}${API}-clang"
  local cxx="$BIN/${clang}${API}-clang++"

  local tl="${target//-/_}"
  local tu="${tl^^}"

  echo "==> $name ($target, API $API, $PROFILE)"

  (
    export "CC_$tl=$cc"
    export "CXX_$tl=$cxx"
    export "AR_$tl=$BIN/llvm-ar"
    export "RANLIB_$tl=$BIN/llvm-ranlib"

    export "CARGO_TARGET_${tu}_LINKER=$cc"
    export "CARGO_TARGET_${tu}_AR=$BIN/llvm-ar"

    if [ "$DEBUG" -eq 1 ]; then
      cargo build --target "$target" -vv
    else
      cargo build --release --target "$target" -vv
    fi
  )
}

# ---- Interactive architecture selection

if [ $# -eq 0 ]; then
  if [ ! -t 0 ]; then
    echo "error: no architecture given" >&2
    list_archs >&2
    exit 2
  fi

  names=()

  for line in "${ARCHS[@]}"; do
    read -r -a w <<<"$line"
    names+=("${w[0]}")
  done

  while true; do
    echo "Which architecture do you want to build?"

    for i in "${!names[@]}"; do
      printf '  %d) %s\n' "$((i + 1))" "${names[$i]}"
    done

    printf '  %d) all\n' "$(( ${#names[@]} + 1 ))"

    read -r -p "Number or name (several separated by spaces, q to quit): " answer || exit 1

    case "$answer" in
      q|Q|quit|exit)
        exit 0
        ;;
    esac

    read -r -a input <<<"$answer"

    [ ${#input[@]} -eq 0 ] && continue

    chosen=()
    ok=1

    for a in "${input[@]}"; do
      if [[ "$a" =~ ^[0-9]+$ ]]; then
        if [ "$a" -ge 1 ] && [ "$a" -le "${#names[@]}" ]; then
          a="${names[$((a - 1))]}"
        elif [ "$a" -eq "$(( ${#names[@]} + 1 ))" ]; then
          a="all"
        fi
      fi

      if [ "$a" != "all" ] && ! resolve "$a" >/dev/null; then
        echo "'$a' is not a supported architecture." >&2
        ok=0
        break
      fi

      chosen+=("$a")
    done

    [ "$ok" -eq 1 ] && break

    echo
  done

  set -- "${chosen[@]}"
fi

# ---- Process architecture arguments

picked=()

for arg in "$@"; do
  case "$arg" in
    -h|--help|--list|list)
      list_archs
      exit 0
      ;;

    all)
      for line in "${ARCHS[@]}"; do
        read -r -a w <<<"$line"
        picked+=("${w[0]}")
      done
      ;;

    *)
      if ! resolve "$arg" >/dev/null; then
        echo "error: unknown architecture '$arg'" >&2
        list_archs >&2
        exit 2
      fi

      picked+=("$arg")
      ;;
  esac
done

# ---- Validate NDK

if [ ! -d "$BIN" ]; then
  echo "error: NDK toolchain not found at $BIN (set NDK=/path/to/android-ndk-rXX)" >&2
  exit 1
fi

# ---- Build architectures

declare -A seen=()
failed=()

for arg in "${picked[@]}"; do
  read -r name target clang < <(resolve "$arg")

  [ -n "${seen[$name]:-}" ] && continue
  seen[$name]=1

  rustup target add "$target" >/dev/null 2>&1 || true

  if build "$name" "$target" "$clang"; then
    echo "    built: target/$target/$PROFILE/tarpi"
  else
    echo "    FAILED: $name" >&2
    failed+=("$name")
  fi
done

# ---- Final status

if [ ${#failed[@]} -gt 0 ]; then
  echo "failed: ${failed[*]}" >&2
  exit 1
fi

