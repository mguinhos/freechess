# Stockfish.js 19.0.1 corresponding source

This archive contains the complete upstream source tree at commit
0d5dd7944f88c817c07a26bc4fb1905d5f9c46d6 (tag v19.0.1), unchanged,
plus the exact lite NNUE build input in src/nn-61e7af4bb97d.nnue and this note.
The delivered engine JavaScript and WebAssembly are unmodified release assets.

## Build the lite single-threaded variant

Install and activate Emscripten 6.0.9 using the official SDK instructions:
https://emscripten.org/docs/getting_started/downloads.html
Also install Node.js/npm, Python 3, make, and a SHA-256 command (shasum or sha256sum).
Emscripten's em++ must be on PATH. From this source directory:

    npm install --ignore-scripts
    node build.js --single-threaded --lite --strict-em-check -f

package.json pins the build dependency Terser to 5.51.2. The build script checks
Emscripten 6.0.9. scripts/net.sh validates the bundled network and reuses it;
no network download is needed for this build input. The outputs are
src/stockfish-19-lite-single.js and src/stockfish-19-lite-single.wasm.
There are no FreeChess engine patches or additional compiler flags.
This recipe follows the pinned upstream build script; it was not rebuilt here.
General-purpose build tools are not included in this source archive.

## Upstream locations and SHA-256

Release: https://github.com/nmrugg/stockfish.js/releases/tag/v19.0.1
Source: https://github.com/nmrugg/stockfish.js/tree/0d5dd7944f88c817c07a26bc4fb1905d5f9c46d6
Original source archive:
https://codeload.github.com/nmrugg/stockfish.js/tar.gz/0d5dd7944f88c817c07a26bc4fb1905d5f9c46d6
SHA-256: e5289a920554bf0069a6601c228e81d957c00522bb31c4343588dc5d7d251879

Network by Chris Bao (sscg13):
https://tests.stockfishchess.org/api/nn/nn-61e7af4bb97d.nnue
SHA-256: 61e7af4bb97d51eeeb25d322916f86513b5cd3a827ce189c98c6e31946f99e5b

Unmodified release assets:
https://github.com/nmrugg/stockfish.js/releases/download/v19.0.1/stockfish-19-lite-single.js
SHA-256: ccb130d7ff29408531c47d859a49f91d59d4f482d05a53eb1a3231a4f624c91a
https://github.com/nmrugg/stockfish.js/releases/download/v19.0.1/stockfish-19-lite-single.wasm
SHA-256: ee4f115bdb2d32cadf96cada104a05b5ec257d7ad6f57582190b49bc71f096de

Stockfish.js engine code is licensed under GNU GPL version 3. Preserve Copying.txt,
AUTHORS, and existing notices when redistributing. The upstream README identifies
its non-engine code separately as MIT licensed. See the upstream README and
individual files for notices. The release assets retain their original banners.
