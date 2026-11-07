#!/bin/bash

export NDK="$PWD/android-ndk-r30"
export BIN="$NDK/toolchains/llvm/prebuilt/linux-x86_64/bin"

export CC="$BIN/armv7a-linux-androideabi24-clang"
export CXX="$BIN/armv7a-linux-androideabi24-clang++"
export AR="$BIN/llvm-ar"
export RANLIB="$BIN/llvm-ranlib"

export CC_armv7_linux_androideabi="$CC"
export CXX_armv7_linux_androideabi="$CXX"
export AR_armv7_linux_androideabi="$AR"

export CARGO_TARGET_ARMV7_LINUX_ANDROIDEABI_LINKER="$CC"

cargo build --release --target armv7-linux-androideabi -vv
