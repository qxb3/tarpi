#!/bin/bash

# Build tarpi for Android (Termux) with the NDK.
#
#   ./arm_build.sh                 # asks which architecture to build
#   ./arm_build.sh aarch64         # one architecture
#   ./arm_build.sh armv7 aarch64   # several, one after the other
#   ./arm_build.sh all             # every architecture in the list
#   ./arm_build.sh --list          # show the supported architectures
#
# NDK=/path/to/android-ndk-rXX and API=24 can be set in the environment.

set -u

NDK="${NDK:-$PWD/android-ndk-r30}"
API="${API:-24}"
BIN="$NDK/toolchains/llvm/prebuilt/linux-x86_64/bin"

# arch name -> "rust target|clang triple prefix"
# The first word of each line is the name you pass; aliases follow it.
#   name   aliases...            rust target               clang prefix
ARCHS=(
  "armv7   armv7a arm armeabi-v7a   armv7-linux-androideabi   armv7a-linux-androideabi"
  "aarch64 arm64 arm64-v8a          aarch64-linux-android     aarch64-linux-android"
  "x86_64  x64 amd64                x86_64-linux-android      x86_64-linux-android"
  "i686    x86 i386                 i686-linux-android        i686-linux-android"
)

# Print "<name> <rust target> <clang prefix>" for an arch name or alias, or fail.
resolve() {
  local want="$1" line words n
  for line in "${ARCHS[@]}"; do
    read -r -a words <<<"$line"
    n=${#words[@]}
    local target="${words[$((n - 2))]}" clang="${words[$((n - 1))]}"
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
    printf '  %-8s (also: %s) -> %s\n' "${words[0]}" "${words[*]:1:$((n - 3))}" "${words[$((n - 2))]}"
  done
  echo "  all      every architecture above"
}

build() {
  local name="$1" target="$2" clang="$3"
  local cc="$BIN/${clang}${API}-clang" cxx="$BIN/${clang}${API}-clang++"
  local tl="${target//-/_}"            # armv7_linux_androideabi
  local tu="${tl^^}"                   # ARMV7_LINUX_ANDROIDEABI

  echo "==> $name ($target, API $API)"
  # A subshell, so one architecture's variables never leak into the next.
  (
    export "CC_$tl=$cc" "CXX_$tl=$cxx" "AR_$tl=$BIN/llvm-ar" "RANLIB_$tl=$BIN/llvm-ranlib"
    export "CARGO_TARGET_${tu}_LINKER=$cc"
    export "CARGO_TARGET_${tu}_AR=$BIN/llvm-ar"
    cargo build --release --target "$target" -vv
  )
}

# ---- arguments
# Nothing passed: ask which architecture(s) to build.
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
      q|Q|quit|exit) exit 0 ;;
    esac
    set -- $answer
    [ $# -eq 0 ] && continue
    # Turn menu numbers into names, then check every word against the list.
    chosen=()
    ok=1
    for a in "$@"; do
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

if [ ! -d "$BIN" ]; then
  echo "error: NDK toolchain not found at $BIN (set NDK=/path/to/android-ndk-rXX)" >&2
  exit 1
fi

# ---- build each one once, in the order given
declare -A seen=()
failed=()
for arg in "${picked[@]}"; do
  read -r name target clang < <(resolve "$arg")
  [ -n "${seen[$name]:-}" ] && continue
  seen[$name]=1
  rustup target add "$target" >/dev/null 2>&1 || true
  if build "$name" "$target" "$clang"; then
    echo "    built: target/$target/release/tarpi"
  else
    echo "    FAILED: $name" >&2
    failed+=("$name")
  fi
done

if [ ${#failed[@]} -gt 0 ]; then
  echo "failed: ${failed[*]}" >&2
  exit 1
fi
