# unarc-cli

A tiny Windows console program Polka runs under Wine to unpack a repack's
FreeArc archives (FitGirl, DODI…) with the `unarc.dll` and CLS compressors
the repack itself ships. `unarc-cli.exe` is built from `unarc-cli.c` and
embedded in Polka (so packaging needs no MinGW):

    i686-w64-mingw32-gcc -O2 -s -static -Wl,--large-address-aware -o unarc-cli.exe unarc-cli.c

32-bit like the `unarc.dll` it loads; large-address-aware so FreeArc's big
dictionaries fit. It also bridges the CLS plugins' named objects (the 64-bit
halves make them `Global\…`, the plugins look for the bare names) and turns
their message boxes into output lines.
