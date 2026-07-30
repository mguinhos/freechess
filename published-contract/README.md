# The published contract

These four files *are* the app's public identity. They are committed, and
`cargo make publish-webapp` publishes from them — it never publishes a freshly
built container.

| File | What it is |
|---|---|
| `web_container_contract.wasm` | the exact bytes behind the address |
| `webapp.params` | the publisher's public key, which is the contract's parameters |
| `contract-id.txt` | the resulting address, and the gate the publish checks itself against |
| `contract-version.txt` | the monotonic release counter |

## Why the WASM is committed rather than rebuilt

A contract key is `BLAKE3(BLAKE3(wasm) ‖ params)`. There is no aliasing and no
redirect at the protocol level: keeping an address means keeping the bytes
*identical*, not equivalent.

Rebuilding cannot promise that. Three things move the bytes, and we have been
bitten by the first:

1. **A dependency bump.** On 2026-07-30 a single transitive patch
   (`hybrid-array 0.4.13 → 0.4.14`) moved the app from `68i7VVAF…` to
   `4FpHYosx…`. The container's tree is 88 crates, 78 of them pulled in by
   `freenet-stdlib`, whose `default = []` — so there are no features to switch
   off and no way to shrink the exposure.
2. **A different machine.** The WASM embeds 34 absolute paths, among them
   `/home/marcel/.cargo/registry/src/…`. Identical source and lock on another
   machine produce a different hash. `--remap-path-prefix` fixes this (verified:
   it removes all 34) but changes the bytes, so it can only ride along with a
   deliberate migration.
3. **A different rustc.** Hence `rust-toolchain.toml`.

Freezing also buys the only escape route. Because the exact bytes are kept, we
can still publish at the old address *after* migrating, to leave a notice
pointing at the new one. Without them the old address can never be written
again, and it simply goes quiet — the failure mode to avoid.

## Why the version counter is committed

The container accepts a state only if its `version` is higher than the one it
holds, so the counter must survive. It used to live under `target/`, which is
gitignored: a `cargo clean` reset it to 1 and every publish afterwards would be
rejected by a network already holding 55.

River hit the matching bug from the other side on 2026-05-16 — they derived the
version from wall-clock time, the on-network value drifted ahead
(30000208 vs. 29649402 from the clock), and publishing was stuck until someone
intervened by hand. A committed counter makes the version a strict local
invariant. Gaps are fine; the contract enforces monotonicity, not contiguity.

`package-webapp` snapshots `contract-version.txt.prev` before bumping, and
`publish-webapp` restores it if the publish fails, so a failed attempt cannot
leave the counter ahead of the network. The read-modify-write is not safe under
two concurrent publishes on one machine — wrap it in `flock` if you ever need
that.

## Changing the address on purpose

`cargo make update-published-contract` rebuilds the container from source,
copies the result here and prints the new address. Do it only when the container
genuinely must change, and in the same commit:

1. Record the outgoing address, so the old bytes stay in git history and remain
   publishable.
2. Publish a migration notice at the **old** address — the admin panel has one;
   it locks new games in the lobby, leaves games in progress alone, and shows
   users where to go.
3. Commit the new files together with the notice.
