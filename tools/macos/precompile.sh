#!/bin/zsh
# Compile les .hlsl laissés par VIctory (VICTORY_HLSL_DIR) avec le DXC de D3DMetal.
cd "${0:A:h:h:h}"; D=target/macos/hlsl; mkdir -p $D
L=~/VIctory-macOS/target/wine-cx/lib/external/D3DMetal.framework/Versions/A/Resources/libdxcompiler.dylib
[[ target/macos/dxc-native -nt tools/macos/dxc-native.cpp ]] || clang++ -std=c++17 -arch x86_64 -o target/macos/dxc-native tools/macos/dxc-native.cpp
for f in $D/*.hlsl(N); do
  n=${f:t:r}; h=${n%%.*}; r=${n#*.}; t=${r%%.*}; e=${r#*.}
  [[ -f $D/$h.dxil ]] || { target/macos/dxc-native $L $f $D/$h.tmp ${t%%_*}_6_0 $e && mv $D/$h.tmp $D/$h.dxil && echo "$h $t $e"; }
done
