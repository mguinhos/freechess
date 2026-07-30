# FreeChess

**<https://github.com/mguinhos/freechess>**

A decentralized chess site built on [Freenet](https://freenet.org). Play, watch,
search and replay games — with no server anywhere in the picture.

Every game, the lobby, the Elo ranking, player presence and the shared history
live in Freenet contracts, replicated across peers. The only thing that never
leaves your device is your signing key.

---

## What it does

- **Play chess.** Full rules: castling, en passant, promotion, checkmate,
  stalemate, threefold repetition, the fifty-move rule, and clocks with
  Fischer increment.
- **Watch any game live.** Every game is public by default. The home page shows
  the current board of *every* game in progress, updating in real time.
- **Search.** Find games by player nickname or game id.
- **Elo ranking.** Ratings are verifiable, not self-declared (see below).
- **Bullet / Blitz / Rapid / Classical**, classified by estimated duration.
- **Presence.** See who is online, away or offline.
- **Challenge someone directly** — including a spectator you noticed watching
  the same game as you.
- **Shareable replays.** Every game gets a link; open it and scrub through the
  moves. Games export as PGN.
- **Portable accounts.** Export your account as a single string and import it on
  another device.
- **Anti-spam.** Creating a game costs proof-of-work, and each player has a
  quota of open games.

## How it works

Freenet's core abstraction is a **contract**: a WebAssembly module that defines
both a piece of public state and the rules for merging concurrent updates to it.
A contract's key is `BLAKE3(BLAKE3(wasm) ‖ params)`, so the data's identity is
bound to the code that governs it — you never have to trust the peer serving it.

FreeChess uses four contracts and one delegate.

| Component | Role |
|---|---|
| `game-contract` | One instance **per game**: the signed move list, the sealed opponent seat, the result. |
| `lobby-contract` | A singleton: live games (each with a signed board snapshot), the Elo ranking, and presence. |
| `archive-contract` | The shared history, sharded into buckets of 1000 games. |
| `player-contract` | One instance per player: nickname and certified game history. |
| `chess-delegate` | Runs locally. Holds the private signing key — the one thing not replicated. |

### One subscription drives the home page

The lobby carries a signed FEN snapshot for every running game, so a client
subscribes to **one contract** and receives live board updates for the whole
site. Opening a specific game adds a second subscription, to that game's own
contract, for the authoritative move list and interactive play.

### Only the two players can move — in that game

Each move carries the mover's key and a signature over
`(game_id, ply, move, timestamp)`. Validation requires that key to be exactly the
one owning the side to move at that ply, where the two keys come from the
contract parameters (the creator) and the sealed opponent slot.

There is no permission list to tamper with. A spectator, a relaying peer, or
anyone else simply has no key that satisfies the check. Because every signature
covers the `game_id`, authority in one game means nothing in another — so a
player can have any number of simultaneous games, each independently governed.

Playing out of turn fails the same check: your key is only accepted on your own
plies.

### Convergence

The platform requires each contract's merge to be an **idempotent commutative
monoid**, so replicas converge no matter what order updates arrive in. Every
conflict here is settled by a *total order over the merged set*, never by arrival
order:

- A **join race** (two people accepting the same open game at once) resolves to
  the earliest `joined_at`, with the key bytes as tiebreak.
- A **double-signed ply** resolves by signature bytes.
- Every **quota and cap** (lobby entries, archive shards, ranking, history) is
  enforced by sorting and truncating, which is a pure function of the merged
  contents.

That last point is load-bearing: an eviction rule that depended on arrival order
would silently diverge peers, which is exactly the failure the whitepaper warns
about.

### Anti-spam

Three independent layers, all verifiable from state alone:

1. **Proof-of-work per game.** A game's `game_id` must be a hash of the
   creator's key and a nonce with 16 leading zero bits. Since `game_id` is a
   contract *parameter*, the work is bound to the contract key itself. Fresh
   keypairs buy an attacker nothing, which matters because Freenet deliberately
   leaves Sybil resistance to the application layer.
2. **A quota per creator:** at most 3 games waiting for an opponent and 20
   listings total.
3. **A global cap** on lobby entries.

### Verifiable Elo

A rating is only meaningful if a peer can check it without trusting whoever
published it — but replaying someone's entire history would mean fetching every
game they ever played.

A **game certificate** closes that gap. It is co-signed by *both* players and
records the rating each held going into the game, so any peer checks the claim in
O(1):

```
claimed_rating == elo(rating_before, opponent_rating, score)
```

Both inputs come from bytes the opponent signed, so you cannot inflate your own
rating without an opponent willing to co-sign an inflated starting point — and
each such step costs a real game with proof-of-work behind it.

The Elo arithmetic is integer-only: floating point would be reproducible in
practice but is not *guaranteed* identical across libm implementations, and a
one-point disagreement between peers is a permanent state divergence.

### History and replay

Certificates carry the full move list in three bytes per move, so a thousand
games fit in well under a megabyte per archive shard. A game's shard is derived
from the game itself:

```
shard = (day = finished_at / 24h, bucket = first byte of game_id % 16)
```

Nothing coordinates this — any peer computes where a game belongs, and where to
look for it, with no index contract and no allocator.

### Presence

Presence is a signed heartbeat with a timestamp. You go offline by *expiry*, not
by sending a goodbye — a peer that crashes or closes the tab cannot publish
anything, so any design requiring a final write would leave ghosts online
forever. This mirrors the approach in Freenet's own `freenet-ping` example, with
two differences: heartbeats are signed (so nobody can forge your presence), and
the map is a `BTreeMap` rather than a `HashMap`, because a `HashMap` in a state
summary serializes in nondeterministic order and makes the core's convergence
check misfire.

## Building

Requires Rust (pinned in `rust-toolchain.toml`), the `wasm32-unknown-unknown`
target, `cargo-make`, the Dioxus CLI, and a Freenet node.

```bash
rustup target add wasm32-unknown-unknown
cargo install cargo-make dioxus-cli --locked

cargo make build        # contracts, delegate, and the web UI
cargo make test         # the full test suite
cargo make publish      # publish to the local node
```

The UI embeds the contract WASM at build time (a client PUTs the contract, not
just its state), so the contracts must be built before the UI — `cargo make`
handles the ordering.

## Testing

```bash
cargo test                          # unit and state tests
cargo test --release -- --ignored   # deep perft
cargo make test-e2e                 # two-node browser test
```

The move generator is verified with **perft** against the five standard
reference positions — including 4,865,609 nodes at depth 5 from the starting
position. This is how you prove a chess engine handles castling, en passant,
promotion and pin interactions correctly; a single wrong edge case shifts the
count.

The state tests cover what actually matters for a decentralized app:
authorization (a spectator cannot move, a player cannot move out of turn,
authority does not leak between games) and convergence (merging in any order,
any number of times, reaches the same state).

## Design notes

**No external requests.** The gateway serves the app under a same-origin CSP, so
there are no CDN scripts, no web fonts and no remote images. Pieces are Unicode
glyphs, the stylesheet ships in the bundle, and the WebSocket URL is derived from
the page's own origin rather than hardcoded.

**No third-party assets**, and therefore no licence obligations beyond this
project's own: the solid Unicode glyph set (U+265A–U+265F) is used for both
colours, with white pieces painted white and outlined in CSS.

**Contracts must not pull in `getrandom`.** They run under wasmtime on
`wasm32-unknown-unknown`, which has no backend for it; any dependency that does
produces wasm-bindgen placeholder imports the host cannot resolve. `chrono`'s
default features are the usual culprit — see the note in the workspace
`Cargo.toml`. Contracts are pure deterministic state transitions and never need
randomness anyway.

## Known limitations

- **Colluding keys can farm each other's ratings.** Two cooperating players can
  co-sign fabricated results between themselves. This is inherent: no local check
  distinguishes a real game between two willing players from a staged one. The
  proof-of-work prices the attack, but does not eliminate it.
- **Nicknames are not unique.** Uniqueness would require a global registry with a
  first-come-first-served race — exactly the global coordination this platform
  avoids. Identity is the player id; the nickname is a label.
- **Your key is your account.** There is no reset and no recovery. Export it.
- **Archive capacity** is 16,000 games per day (1000 per shard × 16 buckets).
  Raising it changes the WASM, and therefore requires a migration entry.

## Credits

Created by **Marcel Guinhos** — <https://github.com/mguinhos/freechess>

Built on [Freenet](https://freenet.org) by the Freenet Project. Architecture
follows the patterns established by [River](https://github.com/freenet/river),
Freenet's reference decentralized chat application, and the
[freenet-agent-skills](https://github.com/freenet/freenet-agent-skills)
`dapp-builder` guide.

## Licence

MIT — see [LICENSE](LICENSE).

## Publishing notes

Two things about publishing are worth knowing before you try it.

**Republishing needs UPDATE, not PUT.** `fdev publish` is a PUT, and a PUT
against a contract that already holds state does not replace it. To ship a new
release of the webapp, PUT once to create it and use
`fdev execute update <key> <state> --as-state` afterwards. The web-container
contract only accepts full-state replacements (a webapp has no meaningful
incremental form), which is what `--as-state` selects.

**Large states do not publish reliably on freenet 0.2.114.** Observed on a clean
two-node network: publishing the 56-byte lobby succeeds and the contract is
retrievable, while publishing the ~400 KB webapp reports
`published successfully` and then is not stored at all — a subsequent
`fdev execute get` returns `Contract not found`, and the gateway serves HTTP 500
with `NotFound` in its log. The same webapp published and served correctly on
0.2.113, where the full browser suite passed across two nodes.

If you hit this, check what the node actually holds rather than trusting the
publish output:

```bash
fdev --port <ws> execute get <contract-id> -o /tmp/state
```
